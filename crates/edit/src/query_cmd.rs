//! What the `element.query`, `element.at` and `selection.select` commands need on top of [`crate::query`].
//!
//! * [`RefQuery`] is the filter as an agent writes it: elements are given as ids or `@name`
//!   (docs/commands.md section 1.3, `Ref`). [`RefQuery::resolve`] turns it into the strict [`Query`] with
//!   [`resolve_ref`], so the matching rules stay in `query`.
//! * [`all_matches`] pages through [`run_query`] until every match is collected.
//! * [`ALL_FIELDS`] lists every field `describe` can produce.
//! * [`hit_members`] lists the elements behind a hit that counts as a whole group.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use drafftink_scene::{ElementId, GroupId, Kind, SceneRead};

use crate::group::units::{Relations, is_label, primary_id, sort_z};
use crate::query::{
    AtHit, ConnectedTo, Field, LinkDirection, MAX_LIMIT, OneOrMany, Order, Query, QueryCtx, QueryError, QueryOpts,
    RegionFilter, StyleFilter, TextFilter, resolve_ref, run_query,
};

/// Every field of `describe`, in the order of the catalog.
pub const ALL_FIELDS: [Field; 14] = [
    Field::Id,
    Field::Kind,
    Field::Name,
    Field::Text,
    Field::Bounds,
    Field::Style,
    Field::Angle,
    Field::GroupIds,
    Field::Frame,
    Field::Bindings,
    Field::BoundText,
    Field::Locked,
    Field::Link,
    Field::Z,
];

/// Arrows bound to an element, and the elements on their other end. The element is an id or `@name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RefConnectedTo {
    /// The element: an id or `@name`.
    #[serde(rename = "ref")]
    pub target: String,
    /// The direction of the arrows.
    #[serde(default)]
    pub direction: LinkDirection,
}

/// The filter of `element.query` with elements given as ids or `@name`. Every given field must match (AND).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct RefQuery {
    /// Only these elements (ids or `@name`).
    pub ids: Option<Vec<String>>,
    /// Text, labels and LaTeX that match.
    pub text: Option<TextFilter>,
    /// One kind or a list of kinds.
    pub kind: Option<OneOrMany<Kind>>,
    /// Style values.
    pub style: Option<StyleFilter>,
    /// A region of the canvas.
    pub region: Option<RegionFilter>,
    /// Connected by arrows to this element.
    pub connected_to: Option<RefConnectedTo>,
    /// Members of this group.
    pub group: Option<GroupId>,
    /// Children of this frame (an id or `@name`).
    pub frame: Option<String>,
    /// The element name, as a glob (`*` any run, `?` one character). Case-sensitive.
    pub name: Option<String>,
    /// Locked or not.
    pub locked: Option<bool>,
    /// In the selection or not.
    pub selected: Option<bool>,
    /// Has a bound label or not.
    pub has_label: Option<bool>,
    /// The label of this container (an id or `@name`).
    pub bound_text_of: Option<String>,
}

impl RefQuery {
    /// The strict query: every id and `@name` resolved to a live element.
    pub fn resolve<R: SceneRead>(&self, scene: &R) -> Result<Query, QueryError> {
        let one = |s: &String| resolve_ref(scene, s);
        Ok(Query {
            ids: self.ids.as_ref().map(|v| v.iter().map(one).collect::<Result<Vec<_>, _>>()).transpose()?,
            text: self.text.clone(),
            kind: self.kind.clone(),
            style: self.style.clone(),
            region: self.region.clone(),
            connected_to: match &self.connected_to {
                Some(c) => Some(ConnectedTo { target: one(&c.target)?, direction: c.direction }),
                None => None,
            },
            group: self.group.clone(),
            frame: self.frame.as_ref().map(one).transpose()?,
            name: self.name.clone(),
            locked: self.locked,
            selected: self.selected,
            has_label: self.has_label,
            bound_text_of: self.bound_text_of.as_ref().map(one).transpose()?,
        })
    }
}

/// Every id that matches `q`, in z-order. Pages through [`run_query`] in steps of [`MAX_LIMIT`].
pub fn all_matches<R: SceneRead>(scene: &R, q: &Query, ctx: &QueryCtx<'_>) -> Result<Vec<ElementId>, QueryError> {
    let mut out: Vec<ElementId> = Vec::new();
    let mut opts = QueryOpts { limit: MAX_LIMIT, offset: 0, order: Order::Z };
    loop {
        let page = run_query(scene, q, ctx, &opts)?;
        let n = page.ids.len();
        out.extend(page.ids);
        if !page.truncated || n == 0 {
            return Ok(out);
        }
        opts.offset = opts.offset.saturating_add(n);
    }
}

/// The elements behind a hit, in z-order: the live members of the hit's group (without bound text) when the
/// hit counts as a whole group, else the element alone.
pub fn hit_members<R: SceneRead>(scene: &R, hit: &AtHit) -> Vec<ElementId> {
    let Some(g) = &hit.group else { return vec![hit.id.clone()] };
    let rel = Relations::build(scene);
    let mut v: Vec<ElementId> = rel
        .members(g)
        .iter()
        .filter(|m| scene.get(m).is_some_and(|e| !is_label(scene, e)))
        .map(|m| primary_id(scene, m))
        .collect();
    v.sort();
    v.dedup();
    sort_z(scene, &mut v);
    v
}
