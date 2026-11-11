//! Reading the scene: `element.query`, `element.at` and `doc.outline`.
//!
//! These are pure reads over [`SceneRead`]. They build the small indexes they need in one pass over the scene, so
//! they work on any store (and on an open transaction) without relying on its derived index.
//!
//! * [`run_query`] filters elements ([`Query`]), orders them and pages the result. [`describe`] turns a result id
//!   into the JSON an agent reads (`fields`).
//! * [`elements_at`] hit-tests a point from the front.
//! * [`outline`] summarises the document as nodes, edges, groups and frames in a stable, reading-friendly order,
//!   with a short text form ([`Outline::to_text`]) meant for an LLM context.

mod path;
mod refs;
mod regex_lite;

pub use path::hit_by_path;
pub use refs::{ALL_FIELDS, RefConnectedTo, RefQuery, all_matches, hit_members};

use std::collections::{BTreeMap, BTreeSet, HashMap};

use drafftink_geom::kurbo::{Point, Rect, Size};
use drafftink_geom::transform::ElementFrame;
use drafftink_geom::{DiamondShape, EllipseShape, HitOpts, OrientedBox, Outline as _, Polyline, RectShape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::target::TargetError;
use drafftink_scene::{
    Color, Element, ElementId, ElementKind, FillPattern, FontFamily, GroupId, Head, Kind, PathType, SceneRead,
    Sloppiness, StrokeStyle, TextAlign,
};

pub use regex_lite::{Case, Regex, RegexError};

// ---------------------------------------------------------------------------------------------
// The filter
// ---------------------------------------------------------------------------------------------

/// One value or a list of values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    /// One value.
    One(T),
    /// A list.
    Many(Vec<T>),
}

impl<T: PartialEq> OneOrMany<T> {
    fn contains(&self, v: &T) -> bool {
        match self {
            OneOrMany::One(x) => x == v,
            OneOrMany::Many(xs) => xs.contains(v),
        }
    }
}

/// Text to look for in the text of elements, their labels and LaTeX. Case-insensitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TextFilter {
    /// A word or phrase the text contains.
    Contains(String),
    /// Either or both: a phrase the text contains, and a regular expression it matches anywhere.
    Spec {
        /// A phrase the text contains.
        #[serde(default)]
        contains: Option<String>,
        /// A regular expression: literals, `.`, classes like `[a-z]`, `* + ?`, `^ $`, `|`, and `\d \w \s`.
        #[serde(default)]
        regex: Option<String>,
    },
}

/// A rectangle as `{x, y, w, h}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RectSpec {
    /// Left.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl RectSpec {
    fn rect(self) -> Rect {
        Rect::new(
            self.x.min(self.x + self.w),
            self.y.min(self.y + self.h),
            self.x.max(self.x + self.w),
            self.y.max(self.y + self.h),
        )
    }
}

/// How an element relates to the region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegionMode {
    /// The element's bounds touch the region (the default).
    #[default]
    Overlap,
    /// The region contains the element completely.
    Contain,
    /// The element contains the region completely.
    ContainedBy,
}

/// Elements in a region of the canvas, by their (rotated) bounding boxes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegionFilter {
    /// The region.
    pub rect: RectSpec,
    /// How the element relates to it.
    #[serde(default)]
    pub mode: RegionMode,
}

/// Which arrows count as a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkDirection {
    /// Arrows that end at the element, and the elements they start at.
    In,
    /// Arrows that start at the element, and the elements they end at.
    Out,
    /// Both (the default).
    #[default]
    Any,
}

/// Arrows bound to an element, and the elements on their other end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectedTo {
    /// The element.
    #[serde(rename = "ref")]
    pub target: ElementId,
    /// The direction of the arrows.
    #[serde(default)]
    pub direction: LinkDirection,
}

/// Exact style values to match. Only the properties that are given are compared.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct StyleFilter {
    /// Stroke colour.
    pub stroke_color: Option<Color>,
    /// Fill colour. `"transparent"` matches elements without a fill.
    pub fill_color: Option<Color>,
    /// Stroke width.
    pub stroke_width: Option<f64>,
    /// Dash pattern.
    pub stroke_style: Option<StrokeStyle>,
    /// Fill pattern.
    pub fill_pattern: Option<FillPattern>,
    /// Sloppiness.
    pub sloppiness: Option<Sloppiness>,
    /// Opacity.
    pub opacity: Option<f64>,
    /// Corner radius.
    pub corner_radius: Option<f64>,
    /// Font family (text only).
    pub font_family: Option<FontFamily>,
    /// Font size (text only).
    pub font_size: Option<f64>,
    /// Text alignment (text only).
    pub text_align: Option<TextAlign>,
}

/// The filter of `element.query`. Every given field must match (AND).
///
/// Labels that are bound to a shape follow their container: they are left out of the result unless the filter asks
/// for kind `text`, names `ids`, or sets `bound_text_of`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Query {
    /// Only these elements.
    pub ids: Option<Vec<ElementId>>,
    /// Text, labels and LaTeX that match.
    pub text: Option<TextFilter>,
    /// One kind or a list of kinds.
    pub kind: Option<OneOrMany<Kind>>,
    /// Style values.
    pub style: Option<StyleFilter>,
    /// A region of the canvas.
    pub region: Option<RegionFilter>,
    /// Connected by arrows to this element.
    pub connected_to: Option<ConnectedTo>,
    /// Members of this group.
    pub group: Option<GroupId>,
    /// Children of this frame.
    pub frame: Option<ElementId>,
    /// The element name, as a glob (`*` any run, `?` one character). Case-sensitive.
    pub name: Option<String>,
    /// Locked or not.
    pub locked: Option<bool>,
    /// In the selection or not.
    pub selected: Option<bool>,
    /// Has a bound label or not.
    pub has_label: Option<bool>,
    /// The label of this container.
    pub bound_text_of: Option<ElementId>,
}

/// Order of the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// Back to front (the default).
    #[default]
    Z,
    /// Top to bottom, then left to right, by the top-left corner of the bounds.
    Reading,
    /// Left to right, then top to bottom.
    Position,
}

/// Paging and ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryOpts {
    /// At most this many ids (1 to 1000).
    pub limit: usize,
    /// Skip this many.
    pub offset: usize,
    /// The order.
    pub order: Order,
}

impl Default for QueryOpts {
    fn default() -> Self {
        Self { limit: 100, offset: 0, order: Order::Z }
    }
}

/// The largest page.
pub const MAX_LIMIT: usize = 1000;

/// What the filter cannot know by itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryCtx<'a> {
    /// The selected elements, for the `selected` filter.
    pub selected: Option<&'a BTreeSet<ElementId>>,
}

/// The matching ids.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct QueryResult {
    /// How many elements match.
    pub total: usize,
    /// The requested page.
    pub ids: Vec<ElementId>,
    /// More matches exist after this page.
    pub truncated: bool,
}

/// Why a query was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum QueryError {
    /// The regular expression is not valid or too big.
    #[error(
        "The text regex `{pattern}` is not valid: {reason}. Fix the pattern, or use `contains` for a plain phrase."
    )]
    BadRegex {
        /// The pattern.
        pattern: String,
        /// What is wrong.
        reason: String,
    },
    /// An element named in the filter does not exist.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// A reference is not an id.
    #[error("`{0}` is not an element id or an @name. Ids use only letters, digits, '_' and '-'.")]
    BadRef(String),
    /// A number in the filter is not finite.
    #[error("The region must have finite numbers for x, y, w and h.")]
    BadRegion,
    /// A `@name` reference matches no element.
    #[error("No element is named `{0}`.")]
    UnknownName(String),
    /// A page size of 0 or more than the maximum.
    #[error("limit must be between 1 and {MAX_LIMIT}.")]
    BadLimit,
}

/// Resolves a reference: an id, or `@name` for the element with that name.
pub fn resolve_ref<R: SceneRead>(r: &R, reference: &str) -> Result<ElementId, QueryError> {
    if let Some(name) = reference.strip_prefix('@') {
        return r
            .iter()
            .find(|e| e.name.as_deref() == Some(name))
            .map(|e| e.id.clone())
            .ok_or_else(|| QueryError::UnknownName(name.to_string()));
    }
    let id = ElementId::new(reference).map_err(|_| QueryError::BadRef(reference.to_string()))?;
    match r.get(&id) {
        Some(e) if !e.deleted => Ok(id),
        _ => Err(QueryError::Target(TargetError::NotFound(id))),
    }
}

/// The axis-aligned bounds of an element, rotation included.
pub fn element_bounds(e: &Element) -> Rect {
    let t = &e.transform;
    OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle).aabb()
}

fn glob_match(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while t < text.len() {
        match (pattern.get(p), text.get(t)) {
            (Some(pc), Some(tc)) if *pc == '?' || pc == tc => {
                p += 1;
                t += 1;
            }
            (Some('*'), _) => {
                star = Some(p);
                mark = t;
                p += 1;
            }
            _ => match star {
                Some(s) => {
                    p = s + 1;
                    mark += 1;
                    t = mark;
                }
                None => return false,
            },
        }
    }
    pattern.get(p..).is_none_or(|rest| rest.iter().all(|c| *c == '*'))
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// The label of every container.
struct Labels {
    by_container: HashMap<ElementId, ElementId>,
}

impl Labels {
    fn build<R: SceneRead>(r: &R) -> Self {
        let mut by_container: HashMap<ElementId, ElementId> = HashMap::new();
        for e in r.iter() {
            if let Some(c) = e.container_id() {
                match by_container.get(c) {
                    Some(old) if *old <= e.id => {}
                    _ => {
                        by_container.insert(c.clone(), e.id.clone());
                    }
                }
            }
        }
        Self { by_container }
    }

    fn text_of<'a, R: SceneRead>(&self, r: &'a R, container: &ElementId) -> Option<&'a str> {
        let l = self.by_container.get(container)?;
        r.get(l).and_then(Element::text_data).map(|t| t.content.as_str())
    }
}

/// The text an element shows: its own, its latex, its frame name, or its label.
fn element_text<'a, R: SceneRead>(r: &'a R, labels: &Labels, e: &'a Element) -> Option<&'a str> {
    match &e.kind {
        ElementKind::Text(t) => Some(t.content.as_str()),
        ElementKind::Math(m) => Some(m.latex.as_str()),
        ElementKind::Frame(f) => f.name.as_deref(),
        _ => labels.text_of(r, &e.id),
    }
}

struct Compiled<'q> {
    q: &'q Query,
    contains: Option<String>,
    regex: Option<Regex>,
    region: Option<(Rect, RegionMode)>,
    connected: Option<BTreeSet<ElementId>>,
    name: Option<Vec<char>>,
    include_bound_text: bool,
}

fn live_exists<R: SceneRead>(r: &R, id: &ElementId) -> bool {
    r.get(id).is_some_and(|e| !e.deleted)
}

fn compile<'q, R: SceneRead>(r: &R, q: &'q Query) -> Result<Compiled<'q>, QueryError> {
    let (mut contains, mut regex) = (None, None);
    match &q.text {
        Some(TextFilter::Contains(s)) => contains = Some(s.to_lowercase()),
        Some(TextFilter::Spec { contains: c, regex: re }) => {
            contains = c.as_ref().map(|s| s.to_lowercase());
            if let Some(p) = re {
                regex = Some(
                    Regex::new(p, Case::Insensitive)
                        .map_err(|e| QueryError::BadRegex { pattern: p.clone(), reason: e.to_string() })?,
                );
            }
        }
        None => {}
    }
    let region = match &q.region {
        Some(f) => {
            let rect = f.rect.rect();
            if ![rect.x0, rect.y0, rect.x1, rect.y1].iter().all(|v| v.is_finite()) {
                return Err(QueryError::BadRegion);
            }
            Some((rect, f.mode))
        }
        None => None,
    };
    let connected = match &q.connected_to {
        Some(c) => {
            if !live_exists(r, &c.target) {
                return Err(QueryError::Target(TargetError::NotFound(c.target.clone())));
            }
            let want_out = matches!(c.direction, LinkDirection::Out | LinkDirection::Any);
            let want_in = matches!(c.direction, LinkDirection::In | LinkDirection::Any);
            let mut set = BTreeSet::new();
            for e in r.iter().filter(|e| e.tag() == Kind::Arrow) {
                let [s, t] = e.binding_targets();
                if s == Some(&c.target) && want_out {
                    set.insert(e.id.clone());
                    set.extend(t.filter(|t| **t != c.target).cloned());
                }
                if t == Some(&c.target) && want_in {
                    set.insert(e.id.clone());
                    set.extend(s.filter(|s| **s != c.target).cloned());
                }
            }
            Some(set)
        }
        None => None,
    };
    for id in [&q.frame, &q.bound_text_of].into_iter().flatten() {
        if !live_exists(r, id) {
            return Err(QueryError::Target(TargetError::NotFound(id.clone())));
        }
    }
    let include_bound_text =
        q.ids.is_some() || q.bound_text_of.is_some() || q.kind.as_ref().is_some_and(|k| k.contains(&Kind::Text));
    Ok(Compiled {
        q,
        contains,
        regex,
        region,
        connected,
        name: q.name.as_ref().map(|n| n.chars().collect()),
        include_bound_text,
    })
}

fn style_matches(f: &StyleFilter, e: &Element) -> bool {
    let s = &e.style;
    if f.stroke_color.is_some_and(|c| c != s.stroke_color) {
        return false;
    }
    if let Some(c) = f.fill_color {
        let ok = if c.a == 0 { s.fill_color.is_none() } else { s.fill_color == Some(c) };
        if !ok {
            return false;
        }
    }
    if f.stroke_width.is_some_and(|v| !same(v, s.stroke_width))
        || f.opacity.is_some_and(|v| !same(v, s.opacity))
        || f.corner_radius.is_some_and(|v| !same(v, s.corner_radius))
        || f.stroke_style.is_some_and(|v| v != s.stroke_style)
        || f.fill_pattern.is_some_and(|v| v != s.fill_pattern)
        || f.sloppiness.is_some_and(|v| v != s.sloppiness)
    {
        return false;
    }
    if f.font_family.is_some() || f.font_size.is_some() || f.text_align.is_some() {
        let Some(t) = e.text_data() else { return false };
        if f.font_family.is_some_and(|v| v != t.font.family)
            || f.font_size.is_some_and(|v| !same(v, t.font.size))
            || f.text_align.is_some_and(|v| v != t.font.align)
        {
            return false;
        }
    }
    true
}

fn matches<R: SceneRead>(r: &R, labels: &Labels, c: &Compiled<'_>, ctx: &QueryCtx<'_>, e: &Element) -> bool {
    let q = c.q;
    if e.container_id().is_some() && !c.include_bound_text {
        return false;
    }
    if q.ids.as_ref().is_some_and(|ids| !ids.contains(&e.id))
        || q.kind.as_ref().is_some_and(|k| !k.contains(&e.tag()))
        || q.locked.is_some_and(|l| e.locked != l)
        || q.group.as_ref().is_some_and(|g| !e.group_ids.contains(g))
        || q.frame.as_ref().is_some_and(|f| e.frame_id.as_ref() != Some(f))
        || q.bound_text_of.as_ref().is_some_and(|of| e.container_id() != Some(of))
        || q.has_label.is_some_and(|h| labels.by_container.contains_key(&e.id) != h)
        || c.connected.as_ref().is_some_and(|conn| !conn.contains(&e.id))
    {
        return false;
    }
    if let Some(sel) = q.selected
        && ctx.selected.is_some_and(|s| s.contains(&e.id)) != sel
    {
        return false;
    }
    if let Some(pat) = &c.name {
        let Some(name) = &e.name else { return false };
        let name: Vec<char> = name.chars().collect();
        if !glob_match(pat, &name) {
            return false;
        }
    }
    if q.style.as_ref().is_some_and(|s| !style_matches(s, e)) {
        return false;
    }
    if let Some((rect, mode)) = &c.region {
        let b = element_bounds(e);
        let ok = match mode {
            RegionMode::Overlap => drafftink_geom::aabb::intersects(&b, rect),
            RegionMode::Contain => drafftink_geom::aabb::contains_rect(rect, &b),
            RegionMode::ContainedBy => drafftink_geom::aabb::contains_rect(&b, rect),
        };
        if !ok {
            return false;
        }
    }
    if c.contains.is_some() || c.regex.is_some() {
        let Some(text) = element_text(r, labels, e) else { return false };
        if c.contains.as_ref().is_some_and(|needle| !text.to_lowercase().contains(needle.as_str())) {
            return false;
        }
        if c.regex.as_ref().is_some_and(|re| !re.is_match(text)) {
            return false;
        }
    }
    true
}

/// Runs a query: the live elements that match, ordered and paged.
pub fn run_query<R: SceneRead>(
    r: &R,
    q: &Query,
    ctx: &QueryCtx<'_>,
    opts: &QueryOpts,
) -> Result<QueryResult, QueryError> {
    if opts.limit == 0 || opts.limit > MAX_LIMIT {
        return Err(QueryError::BadLimit);
    }
    let compiled = compile(r, q)?;
    let labels = Labels::build(r);
    let mut hits: Vec<&Element> = r.iter().filter(|e| matches(r, &labels, &compiled, ctx, e)).collect();
    match opts.order {
        Order::Z => {}
        Order::Reading => hits.sort_by(|a, b| {
            let (ra, rb) = (element_bounds(a), element_bounds(b));
            ra.y0.total_cmp(&rb.y0).then(ra.x0.total_cmp(&rb.x0)).then(a.id.cmp(&b.id))
        }),
        Order::Position => hits.sort_by(|a, b| {
            let (ra, rb) = (element_bounds(a), element_bounds(b));
            ra.x0.total_cmp(&rb.x0).then(ra.y0.total_cmp(&rb.y0)).then(a.id.cmp(&b.id))
        }),
    }
    let total = hits.len();
    let ids: Vec<ElementId> = hits.into_iter().skip(opts.offset).take(opts.limit).map(|e| e.id.clone()).collect();
    let truncated = opts.offset.saturating_add(ids.len()) < total;
    Ok(QueryResult { total, ids, truncated })
}

// ---------------------------------------------------------------------------------------------
// Describing an element
// ---------------------------------------------------------------------------------------------

/// A field of `element.query` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// The id.
    Id,
    /// The kind.
    Kind,
    /// The name.
    Name,
    /// The text, label or LaTeX.
    Text,
    /// The bounds as `{x, y, w, h}`.
    Bounds,
    /// The full style.
    Style,
    /// The angle in degrees.
    Angle,
    /// The groups, innermost first.
    GroupIds,
    /// The frame.
    Frame,
    /// Arrow bindings `{start, end}`.
    Bindings,
    /// The id of the bound label.
    BoundText,
    /// Locked.
    Locked,
    /// The hyperlink or element link (`null` when none).
    Link,
    /// The z-order key.
    Z,
    /// The points of a line or arrow in world coordinates, `[[x, y], ...]`; `null` for other kinds.
    Points,
}

/// The default fields of `element.query`.
pub const DEFAULT_FIELDS: [Field; 5] = [Field::Id, Field::Kind, Field::Name, Field::Text, Field::Bounds];

fn round2(v: f64) -> f64 {
    if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 }
}

/// The JSON objects for the elements `ids` with the chosen fields (`null` for an unknown id).
pub fn describe_all<R: SceneRead>(r: &R, ids: &[ElementId], fields: &[Field]) -> Vec<Value> {
    let labels = Labels::build(r);
    ids.iter().map(|id| r.get(id).map_or(Value::Null, |e| describe_with(r, &labels, e, fields))).collect()
}

/// The JSON object for one element. `Null` for an unknown id.
pub fn describe<R: SceneRead>(r: &R, id: &ElementId, fields: &[Field]) -> Value {
    describe_all(r, std::slice::from_ref(id), fields).into_iter().next().unwrap_or(Value::Null)
}

fn describe_with<R: SceneRead>(r: &R, labels: &Labels, e: &Element, fields: &[Field]) -> Value {
    let mut o = serde_json::Map::new();
    for f in fields {
        let (key, v) = match f {
            Field::Id => ("id", json!(e.id)),
            Field::Kind => ("kind", json!(e.tag().name())),
            Field::Name => ("name", json!(e.name)),
            Field::Text => ("text", json!(element_text(r, labels, e))),
            Field::Bounds => {
                let b = element_bounds(e);
                (
                    "bounds",
                    json!({"x": round2(b.x0), "y": round2(b.y0), "w": round2(b.width()), "h": round2(b.height())}),
                )
            }
            Field::Style => ("style", serde_json::to_value(&e.style).unwrap_or(Value::Null)),
            Field::Angle => ("angle", json!(round2(e.transform.angle.to_degrees()))),
            Field::GroupIds => ("group_ids", json!(e.group_ids)),
            Field::Frame => ("frame", json!(e.frame_id)),
            Field::Bindings => {
                let [s, t] = e.binding_targets();
                ("bindings", json!({"start": s, "end": t}))
            }
            Field::BoundText => ("bound_text", json!(labels.by_container.get(&e.id))),
            Field::Locked => ("locked", json!(e.locked)),
            Field::Link => ("link", json!(e.link)),
            Field::Z => ("z", json!(e.index.as_str())),
            Field::Points => (
                "points",
                json!(
                    crate::linear::world_points(e)
                        .map(|pts| { pts.iter().map(|p| [round2(p.x), round2(p.y)]).collect::<Vec<_>>() })
                ),
            ),
        };
        o.insert(key.to_string(), v);
    }
    Value::Object(o)
}

// ---------------------------------------------------------------------------------------------
// element.at
// ---------------------------------------------------------------------------------------------

/// Options of [`elements_at`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtOpts {
    /// Reach around outlines in world units (the screen tolerance divided by the zoom).
    pub tolerance: f64,
    /// Every hit, top to bottom, instead of only the topmost.
    pub all: bool,
    /// Members of groups are hit as themselves instead of as their group.
    pub deep: bool,
    /// Locked elements can be hit.
    pub include_locked: bool,
}

impl Default for AtOpts {
    fn default() -> Self {
        Self { tolerance: 6.0, all: false, deep: false, include_locked: false }
    }
}

/// An element under the point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AtHit {
    /// The element (for a label: its container).
    pub id: ElementId,
    /// The outermost group when the hit counts as the whole group (not `deep`).
    pub group: Option<GroupId>,
}

fn has_fill(e: &Element) -> bool {
    e.style.fill_color.is_some_and(|c| c.a > 0)
}

fn hit_element(e: &Element, p: Point, tol: f64) -> bool {
    let t = &e.transform;
    let frame = OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle);
    let sw = e.style.stroke_width;
    let opts = |filled: bool| if filled { HitOpts::filled(tol, sw) } else { HitOpts::stroke(tol, sw) };
    let polyline = |points: &[Point], closed: bool| {
        let f = ElementFrame::new(Point::new(t.x, t.y), Size::new(t.w, t.h), t.angle);
        Polyline::from_frame(&f, points, closed).hit_test(p, opts(closed && has_fill(e)))
    };
    match &e.kind {
        ElementKind::Rectangle => {
            RectShape::new(frame).with_corner_radius(e.style.corner_radius).hit_test(p, opts(has_fill(e)))
        }
        ElementKind::Diamond => {
            DiamondShape::new(frame).with_corner_radius(e.style.corner_radius).hit_test(p, opts(has_fill(e)))
        }
        ElementKind::Ellipse => EllipseShape::new(frame).hit_test(p, opts(has_fill(e))),
        ElementKind::Line(d) | ElementKind::Arrow(d) => polyline(&d.points, d.closed),
        ElementKind::Freehand(d) => polyline(&d.points, d.closed),
        ElementKind::Text(_) | ElementKind::Math(_) | ElementKind::Image(_) => {
            RectShape::new(frame).hit_test(p, HitOpts::filled(tol, 0.0))
        }
        // A frame is hit on its outline only.
        ElementKind::Frame(_) => RectShape::new(frame).hit_test(p, HitOpts::stroke(tol, sw.max(1.0))),
    }
}

/// The elements under `p`, from the front. A label counts as its container.
pub fn elements_at<R: SceneRead>(r: &R, p: Point, opts: &AtOpts) -> Vec<AtHit> {
    if !p.x.is_finite() || !p.y.is_finite() {
        return Vec::new();
    }
    let tol = if opts.tolerance.is_finite() { opts.tolerance.max(0.0) } else { 0.0 };
    let mut group_size: HashMap<&GroupId, usize> = HashMap::new();
    for e in r.iter() {
        for g in &e.group_ids {
            *group_size.entry(g).or_default() += 1;
        }
    }
    let all: Vec<&Element> = r.iter().collect();
    let mut out: Vec<AtHit> = Vec::new();
    let mut seen: BTreeSet<ElementId> = BTreeSet::new();
    for e in all.into_iter().rev() {
        if (e.locked && !opts.include_locked) || !hit_element(e, p, tol) {
            continue;
        }
        let target = e.container_id().and_then(|c| r.get(c)).filter(|c| !c.deleted).unwrap_or(e);
        if (target.locked && !opts.include_locked) || !seen.insert(target.id.clone()) {
            continue;
        }
        let group = if opts.deep {
            None
        } else {
            target.group_ids.iter().rev().find(|g| group_size.get(g).copied().unwrap_or(0) >= 2).cloned()
        };
        out.push(AtHit { id: target.id.clone(), group });
        if !opts.all {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// doc.outline
// ---------------------------------------------------------------------------------------------

/// A shape, text or other thing in the outline.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct OutlineNode {
    /// Element id.
    pub id: ElementId,
    /// The `name` an agent can address it by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The kind.
    pub kind: Kind,
    /// The text inside (bound label, own text or LaTeX).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `[x, y, w, h]`, rounded to whole units.
    pub bounds: [i64; 4],
    /// The frame it is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<ElementId>,
    /// Locked.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

/// An arrow.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct OutlineEdge {
    /// The arrow's id.
    pub id: ElementId,
    /// The `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the start is bound to. `None`: loose.
    pub from: Option<ElementId>,
    /// What the end is bound to. `None`: loose.
    pub to: Option<ElementId>,
    /// The arrow's bound text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Marker at the start.
    #[serde(skip_serializing_if = "is_no_head")]
    pub start_head: Head,
    /// Marker at the end.
    #[serde(skip_serializing_if = "is_no_head")]
    pub end_head: Head,
    /// Straight, curved or elbow.
    pub path: PathType,
}

fn is_no_head(h: &Head) -> bool {
    *h == Head::None
}

/// A group with at least two live members.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct OutlineGroup {
    /// Group id.
    pub id: GroupId,
    /// Member ids in outline order (labels follow their containers and are not listed).
    pub members: Vec<ElementId>,
}

/// A frame and what is in it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct OutlineFrame {
    /// Element id.
    pub id: ElementId,
    /// The title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `[x, y, w, h]`.
    pub bounds: [i64; 4],
    /// Child ids: nodes and arrows, in outline order.
    pub children: Vec<ElementId>,
}

/// Said when the outline is cut short.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Truncation {
    /// Nodes in scope (shapes and loose texts).
    pub nodes_total: usize,
    /// Nodes shown.
    pub nodes_shown: usize,
    /// Edges in scope.
    pub edges_total: usize,
    /// Edges shown.
    pub edges_shown: usize,
    /// What to do about it.
    pub hint: String,
}

/// The structure of the document for an agent.
#[derive(Debug, Clone, PartialEq, Default, Serialize, JsonSchema)]
pub struct Outline {
    /// Shapes, images, math, lines and freehand strokes, in reading order.
    pub nodes: Vec<OutlineNode>,
    /// Arrows with their ends.
    pub edges: Vec<OutlineEdge>,
    /// Free text that no arrow connects and that labels nothing.
    pub texts: Vec<OutlineNode>,
    /// Groups.
    pub groups: Vec<OutlineGroup>,
    /// Frames.
    pub frames: Vec<OutlineFrame>,
    /// Present when something was left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<Truncation>,
}

/// What part of the document the outline covers.
#[derive(Debug, Clone, Default)]
pub struct OutlineScope {
    /// Only these elements (the selection).
    pub only: Option<BTreeSet<ElementId>>,
    /// Only elements whose bounds touch this rectangle (the visible area).
    pub region: Option<Rect>,
    /// Only the children of this frame.
    pub frame: Option<ElementId>,
}

impl OutlineScope {
    fn is_whole(&self) -> bool {
        self.only.is_none() && self.region.is_none() && self.frame.is_none()
    }

    fn covers(&self, e: &Element) -> bool {
        self.only.as_ref().is_none_or(|o| o.contains(&e.id))
            && self.frame.as_ref().is_none_or(|f| e.frame_id.as_ref() == Some(f))
            && self.region.as_ref().is_none_or(|r| drafftink_geom::aabb::intersects(&element_bounds(e), r))
    }
}

/// Options of [`outline`].
#[derive(Debug, Clone)]
pub struct OutlineOpts {
    /// Show at most this many nodes and as many edges.
    pub max_nodes: usize,
    /// The part of the document.
    pub scope: OutlineScope,
}

impl Default for OutlineOpts {
    fn default() -> Self {
        Self { max_nodes: 500, scope: OutlineScope::default() }
    }
}

fn rounded(r: Rect) -> [i64; 4] {
    let f = |v: f64| if v.is_finite() { v.round() as i64 } else { 0 };
    [f(r.x0), f(r.y0), f(r.width()), f(r.height())]
}

type Keyed = (OutlineNode, (i64, i64));

/// Builds the outline.
pub fn outline<R: SceneRead>(r: &R, opts: &OutlineOpts) -> Outline {
    let labels = Labels::build(r);
    let live: Vec<&Element> = r.iter().collect();
    let scope = &opts.scope;

    // Texts that an arrow is bound to are nodes; the others are loose notes.
    let mut bound_by_arrow: BTreeSet<&ElementId> = BTreeSet::new();
    for e in live.iter().filter(|e| e.tag() == Kind::Arrow) {
        bound_by_arrow.extend(e.binding_targets().into_iter().flatten());
    }

    let mut nodes: Vec<Keyed> = Vec::new();
    let mut texts: Vec<Keyed> = Vec::new();
    let mut frames_raw: Vec<&Element> = Vec::new();
    for e in &live {
        if e.container_id().is_some() || e.tag() == Kind::Arrow {
            continue;
        }
        if e.tag() == Kind::Frame {
            if scope.is_whole() || scope.covers(e) || scope.frame.as_ref() == Some(&e.id) {
                frames_raw.push(e);
            }
            continue;
        }
        if !scope.covers(e) {
            continue;
        }
        let b = element_bounds(e);
        let label = match &e.kind {
            ElementKind::Text(t) => Some(t.content.clone()),
            ElementKind::Math(m) => Some(m.latex.clone()),
            _ => labels.text_of(r, &e.id).map(str::to_string),
        };
        let node = OutlineNode {
            id: e.id.clone(),
            name: e.name.clone(),
            kind: e.tag(),
            label,
            bounds: rounded(b),
            frame: e.frame_id.clone(),
            locked: e.locked,
        };
        let key = (((b.y0 + b.y1) / 2.0).round() as i64, ((b.x0 + b.x1) / 2.0).round() as i64);
        if e.tag() == Kind::Text && !bound_by_arrow.contains(&e.id) {
            texts.push((node, key));
        } else {
            nodes.push((node, key));
        }
    }
    let by_reading = |a: &Keyed, b: &Keyed| a.1.cmp(&b.1).then(a.0.id.cmp(&b.0.id));
    nodes.sort_by(by_reading);
    texts.sort_by(by_reading);

    // Edges, ordered by the reading rank of their ends.
    let rank: HashMap<&ElementId, usize> = nodes.iter().enumerate().map(|(i, (n, _))| (&n.id, i)).collect();
    let mut edges: Vec<(OutlineEdge, (usize, usize))> = Vec::new();
    for e in live.iter().filter(|e| e.tag() == Kind::Arrow) {
        let Some(d) = e.linear_data() else { continue };
        let [s, t] = e.binding_targets();
        let ends = [s, t];
        let ends_shown = ends.iter().flatten().all(|x| rank.contains_key(*x)) && ends.iter().any(Option::is_some);
        if !(scope.is_whole() || scope.covers(e) || ends_shown) {
            continue;
        }
        let key = (
            s.and_then(|x| rank.get(x)).copied().unwrap_or(usize::MAX),
            t.and_then(|x| rank.get(x)).copied().unwrap_or(usize::MAX),
        );
        edges.push((
            OutlineEdge {
                id: e.id.clone(),
                name: e.name.clone(),
                from: s.cloned(),
                to: t.cloned(),
                label: labels.text_of(r, &e.id).map(str::to_string),
                start_head: d.start_head,
                end_head: d.end_head,
                path: d.path,
            },
            key,
        ));
    }
    edges.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.id.cmp(&b.0.id)));

    let (nodes_total, edges_total) = (nodes.len() + texts.len(), edges.len());
    let max = opts.max_nodes.max(1);
    let mut shown_nodes: Vec<OutlineNode> = nodes.into_iter().map(|(n, _)| n).collect();
    let mut shown_texts: Vec<OutlineNode> = texts.into_iter().map(|(n, _)| n).collect();
    let mut shown_edges: Vec<OutlineEdge> = edges.into_iter().map(|(e, _)| e).collect();
    let mut truncated = None;
    if nodes_total > max || edges_total > max {
        shown_nodes.truncate(max);
        shown_texts.truncate(max.saturating_sub(shown_nodes.len()));
        shown_edges.truncate(max);
        truncated = Some(Truncation {
            nodes_total,
            nodes_shown: shown_nodes.len() + shown_texts.len(),
            edges_total,
            edges_shown: shown_edges.len(),
            hint: format!(
                "The outline is cut at max_nodes={max}. Narrow it with scope (selection, visible or a frame), or raise max_nodes."
            ),
        });
    }

    // Groups and frames, from what is shown.
    let order: HashMap<&ElementId, usize> = shown_nodes
        .iter()
        .map(|n| &n.id)
        .chain(shown_texts.iter().map(|n| &n.id))
        .chain(shown_edges.iter().map(|e| &e.id))
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect();
    let mut group_members: BTreeMap<&GroupId, Vec<&ElementId>> = BTreeMap::new();
    let mut live_group_count: HashMap<&GroupId, usize> = HashMap::new();
    for e in &live {
        for g in &e.group_ids {
            *live_group_count.entry(g).or_default() += 1;
            if e.container_id().is_none() && order.contains_key(&e.id) {
                group_members.entry(g).or_default().push(&e.id);
            }
        }
    }
    let rank_of = |id: &&ElementId| order.get(*id).copied().unwrap_or(usize::MAX);
    let mut groups: Vec<OutlineGroup> = group_members
        .into_iter()
        .filter(|(g, _)| live_group_count.get(g).copied().unwrap_or(0) >= 2)
        .map(|(g, mut m)| {
            m.sort_by_key(rank_of);
            OutlineGroup { id: g.clone(), members: m.into_iter().cloned().collect() }
        })
        .collect();
    groups.sort_by_key(|g| g.members.first().and_then(|m| order.get(m)).copied().unwrap_or(usize::MAX));

    let mut frames: Vec<OutlineFrame> = frames_raw
        .into_iter()
        .map(|f| {
            let mut children: Vec<&ElementId> = live
                .iter()
                .filter(|e| e.frame_id.as_ref() == Some(&f.id) && e.container_id().is_none())
                .map(|e| &e.id)
                .filter(|id| order.contains_key(*id))
                .collect();
            children.sort_by_key(rank_of);
            let name = match &f.kind {
                ElementKind::Frame(d) => d.name.clone(),
                _ => None,
            };
            OutlineFrame {
                id: f.id.clone(),
                name,
                bounds: rounded(element_bounds(f)),
                children: children.into_iter().cloned().collect(),
            }
        })
        .collect();
    frames.sort_by(|a, b| (a.bounds[1], a.bounds[0], &a.id).cmp(&(b.bounds[1], b.bounds[0], &b.id)));

    Outline { nodes: shown_nodes, edges: shown_edges, texts: shown_texts, groups, frames, truncated }
}

const LABEL_CHARS: usize = 80;

fn quote(s: &str) -> String {
    let cut: String = if s.chars().count() > LABEL_CHARS {
        s.chars().take(LABEL_CHARS).chain(std::iter::once('…')).collect()
    } else {
        s.to_string()
    };
    serde_json::to_string(&cut).unwrap_or_else(|_| "\"\"".to_string())
}

impl OutlineNode {
    fn line(&self) -> String {
        let mut s = self.id.to_string();
        if let Some(n) = &self.name {
            s.push_str(&format!(" @{n}"));
        }
        if let Some(l) = &self.label {
            s.push(' ');
            s.push_str(&quote(l));
        }
        let [x, y, w, h] = self.bounds;
        s.push_str(&format!(" {} ({x},{y},{w}x{h})", self.kind.name()));
        if self.locked {
            s.push_str(" locked");
        }
        s
    }
}

impl Outline {
    /// True when the document has nothing to show.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty() && self.texts.is_empty() && self.frames.is_empty()
    }

    /// The short text form: one line per node and per edge, in a fixed order. Meant to be pasted into a prompt.
    ///
    /// ```text
    /// nodes (2):
    /// a "Start" rectangle (0,0,120x60)
    /// b "End" rectangle (0,140,120x60)
    /// edges (1):
    /// a -> b "yes" [e9]
    /// ```
    ///
    /// An edge line ends with the arrow's own id in brackets. `<-`, `<->` and `--` show other head settings, and
    /// `?` stands for an end that is not bound to anything.
    pub fn to_text(&self) -> String {
        if self.is_empty() {
            return "empty document: no elements.\n".to_string();
        }
        let mut out = String::new();
        if !self.nodes.is_empty() {
            out.push_str(&format!("nodes ({}):\n", self.nodes.len()));
            for n in &self.nodes {
                out.push_str(&n.line());
                out.push('\n');
            }
        }
        if !self.edges.is_empty() {
            out.push_str(&format!("edges ({}):\n", self.edges.len()));
            for e in &self.edges {
                let end = |x: &Option<ElementId>| x.as_ref().map_or("?".to_string(), ToString::to_string);
                let arrow = match (e.start_head != Head::None, e.end_head != Head::None) {
                    (false, true) => "->",
                    (true, false) => "<-",
                    (true, true) => "<->",
                    (false, false) => "--",
                };
                out.push_str(&format!("{} {arrow} {}", end(&e.from), end(&e.to)));
                if let Some(l) = &e.label {
                    out.push(' ');
                    out.push_str(&quote(l));
                }
                match e.path {
                    PathType::Sharp => {}
                    PathType::Round => out.push_str(" round"),
                    PathType::Elbow => out.push_str(" elbow"),
                }
                if let Some(n) = &e.name {
                    out.push_str(&format!(" @{n}"));
                }
                out.push_str(&format!(" [{}]\n", e.id));
            }
        }
        if !self.texts.is_empty() {
            out.push_str(&format!("texts ({}):\n", self.texts.len()));
            for n in &self.texts {
                out.push_str(&n.line());
                out.push('\n');
            }
        }
        if !self.groups.is_empty() {
            out.push_str(&format!("groups ({}):\n", self.groups.len()));
            for g in &self.groups {
                let members: Vec<String> = g.members.iter().map(ToString::to_string).collect();
                out.push_str(&format!("{}: {}\n", g.id, members.join(", ")));
            }
        }
        if !self.frames.is_empty() {
            out.push_str(&format!("frames ({}):\n", self.frames.len()));
            for f in &self.frames {
                let [x, y, w, h] = f.bounds;
                out.push_str(&f.id.to_string());
                if let Some(n) = &f.name {
                    out.push(' ');
                    out.push_str(&quote(n));
                }
                let children: Vec<String> = f.children.iter().map(ToString::to_string).collect();
                out.push_str(&format!(" ({x},{y},{w}x{h}): {}\n", children.join(", ")));
            }
        }
        if let Some(t) = &self.truncated {
            out.push_str(&format!(
                "... showing {} of {} nodes and {} of {} edges. {}\n",
                t.nodes_shown, t.nodes_total, t.edges_shown, t.edges_total, t.hint
            ));
        }
        out
    }
}
