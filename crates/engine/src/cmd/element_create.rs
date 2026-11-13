//! `element` family, creation half: `element.create`, `element.connect`, `element.relabel`,
//! `element.set_name` (docs/commands.md §2.4).
//!
//! The commands validate their arguments and delegate. The rules (sizes, defaults of the old tools, names, binding
//! of new arrow ends, frame membership, placement, labels) are in `drafftink_edit::create`,
//! `drafftink_edit::arrow`, `drafftink_edit::binding`, `drafftink_edit::frame` and `drafftink_edit::text_bind`.
//!
//! Text and labels are measured by `Session::services` (the typesetter in the app, the monospace fake in tests);
//! `style` is `drafftink_edit::style::StyleProps` over the session's style defaults.

use drafftink_edit::create::{
    self, AutoBind, ConnectSpec, CreateError, DEFAULT_MATH, DEFAULT_SIMPLIFY, EndBinding, FrameChoice, Joined,
    LinearSpec, PresetId, StrokeSpec, TextPlace, TextSpec,
};
use drafftink_edit::frame;
use drafftink_edit::style::{self, Prop, StyleDefaults, StyleError, StyleProps, applies};
use drafftink_edit::text_bind::{self, Relabeled};
use drafftink_geom::kurbo::{Point, Size};
use drafftink_scene::words::Article;
use drafftink_scene::limits::{MAX_LATEX_CHARS, MAX_POINTS, MAX_TEXT_CHARS};
use drafftink_scene::{BindMode, ElementId, Head, Kind, PathType, PressureMode, SceneRead, SceneTx, Style};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;

use super::element_common::{edit_with_measure, id_strings, prune_selection, select_exactly};
use super::view::{Pt, RectArg};
use crate::cmd::element_common::target_refusal;
use crate::args;
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo};
use crate::state::{BASE_ZOOM, Tool};

// ---------------------------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------------------------

/// An arrow end bound to an element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BindArg {
    /// The element to attach to: an id or `@name`. Bindable kinds: rectangle, diamond, ellipse, text, image, frame.
    pub target: Ref,
    /// `orbit` (default): the end sits on the target's outline plus a small gap. `inside`: the end stays inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<BindMode>,
    /// Where on the target, as ratios `[rx, ry]` of its unrotated box. Default: the outline point facing the
    /// other end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_point: Option<[f64; 2]>,
}

/// How automatic binding of arrow ends works (`bind` of `element.create`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindPolicy {
    /// An end inside a bindable element binds inside; an end near its outline binds on the outline.
    Auto,
    /// Automatic binding whatever the setting `arrow.binding` says (a pointer gesture sends this when Ctrl or Cmd
    /// is held while the setting is off).
    On,
    /// No automatic binding. Ends named in `start` or `end` are still bound.
    Off,
    /// An end over a bindable element binds inside it (Alt held).
    Inside,
}

/// Which frame a new element joins: `"auto"`, an element reference, or `null`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum FrameArg {
    /// The frame under the new element's centre.
    #[default]
    Auto,
    /// No frame.
    NoFrame,
    /// This frame (an id or `@name`).
    Frame(Ref),
}

impl FrameArg {
    pub(super) fn is_auto(&self) -> bool {
        *self == FrameArg::Auto
    }
}

impl Serialize for FrameArg {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            FrameArg::Auto => s.serialize_str("auto"),
            FrameArg::NoFrame => s.serialize_none(),
            FrameArg::Frame(r) => s.serialize_str(&r.0),
        }
    }
}

impl<'de> Deserialize<'de> for FrameArg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(d)? {
            None => FrameArg::NoFrame,
            Some(s) if s == "auto" => FrameArg::Auto,
            Some(s) => FrameArg::Frame(Ref(s)),
        })
    }
}

impl JsonSchema for FrameArg {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FrameArg".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": ["string", "null"],
            "description": "\"auto\" (default): the frame under the new element's centre, if any. An id or \"@name\": that frame. null: no frame."
        })
    }
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// The id of a created element, for the output of `element.create`.
fn rect_out(e: &drafftink_scene::Element) -> RectArg {
    RectArg { x: e.transform.x, y: e.transform.y, w: e.transform.w, h: e.transform.h }
}

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// The text for `empty_text`, as the command's row words it.
fn empty_text(cmd: &str) -> Refusal {
    let msg = if cmd == "element.create" {
        "A text element cannot be empty. Give `text`, or start typing with `text.edit_begin`."
    } else {
        "A text element cannot be empty. Delete it with `element.delete`."
    };
    Refusal::custom("empty_text", cmd, msg)
}

/// One catalog refusal per `CreateError` (no blanket conversion).
pub(super) fn refuse(cmd: &str, e: CreateError) -> Refusal {
    match e {
        CreateError::TooSmall { kind, width, height } => {
            let linear = kind.is_linear();
            let dims = match (width, height) {
                (true, true) => "width and height",
                (true, false) => "width",
                _ => "height",
            };
            let tail = if linear {
                "Give points that are at least 1 unit apart."
            } else {
                "Give bounds with w and h of at least 1."
            };
            Refusal::custom("too_small", cmd, format!("The {kind} is smaller than 1 unit in {dims}. {tail}"))
        }
        CreateError::TooFewPoints { min, have, .. } => Refusal::too_few(cmd, min, "points", have),
        CreateError::IdInUse(id) => Refusal::custom(
            "id_in_use",
            cmd,
            format!("The id `{}` is already used by another element. Omit `id` to get a fresh one.", crate::refusal::echo(id.as_str())),
        ),
        CreateError::BadName => bad_name(cmd),
        CreateError::NameTaken { name, owner } => Refusal::custom(
            "name_taken",
            cmd,
            format!(
                "The name `{}` is already used by element `{owner}`. Names must be unique so that `@{}` is unambiguous. Pick another name, or clear the other with `element.set_name`.",
                crate::refusal::echo(&name),
                crate::refusal::echo(&name)
            ),
        )
        .with_details(json!({ "name": crate::refusal::echo(&name), "owner": owner.as_str() })),
        CreateError::Target(t) => target_refusal(cmd, t),
        CreateError::NotBindable { id, kind, label } => {
            let text = if label {
                "Text that labels another element cannot be an arrow target. Bind to the labelled element instead.".to_owned()
            } else {
                let mut k = kind.name().to_owned();
                if let Some(first) = k.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                format!("{k} elements cannot be arrow targets. Bindable kinds: rectangle, diamond, ellipse, text, image, frame.")
            };
            Refusal::custom("not_bindable", cmd, text).with_details(json!({ "ids": [id.as_str()] }))
        }
        CreateError::SameElement => Refusal::custom(
            "same_element",
            cmd,
            "Cannot connect an element to itself with `element.connect`. Pass two different elements.",
        ),
        CreateError::NotLabelContainer { id, kind } => Refusal::custom(
            "wrong_kind",
            cmd,
            format!("`{id}` is {} and cannot hold a label. Labels go on rectangles, diamonds, ellipses and arrows.", Article::new(kind.name())),
        ),
        CreateError::AlreadyLabelled { container, existing } => Refusal::custom(
            "already_bound",
            cmd,
            format!("`{container}` already has a label (`{existing}`). Remove it first with `text.unbind`."),
        ),
        CreateError::EmptyText => empty_text(cmd),
        CreateError::NotAFrame { id, kind } => {
            Refusal::custom("wrong_kind", cmd, format!("`{id}` is {}, not a frame. Pass the id of a frame, \"auto\" or null as `frame`.", Article::new(kind.name())))
        }
        CreateError::NestedFrame => Refusal::bad_arg(cmd, "frame", "a frame cannot be placed inside another frame"),
        CreateError::TextTooLong { n, max } => Refusal::limit_exceeded(cmd, n, "characters", max),
        CreateError::NotFinite => Refusal::bad_arg(cmd, "at", "must be finite numbers within the document limits"),
        e @ CreateError::Invalid { .. } => Refusal::internal(cmd, e.to_string()),
        CreateError::Internal(m) => Refusal::internal(cmd, m),
    }
}

fn bad_name(cmd: &str) -> Refusal {
    Refusal::custom(
        "bad_name",
        cmd,
        "A name must be 1 to 64 characters, must not start with `@` or `$`, and must not contain control characters.",
    )
}

// ---------------------------------------------------------------------------------------------
// Formulas
// ---------------------------------------------------------------------------------------------

/// The box of a formula, or the `invalid_latex` refusal of the catalog. A session with the fake services
/// (tests) has no formula engine and answers with the estimate of the old app: width `chars * size * 0.5`,
/// height `size` plus a depth of `0.3 * size`.
fn math_box(s: &Session, latex: &str, font_size: f64) -> Result<Size, Refusal> {
    match s.services.io().math_size(latex, font_size) {
        Some(b) => Ok(Size::new(b.width.max(1.0), (b.height + b.depth).max(1.0))),
        None => {
            let why = match s.services.math_layout(latex, font_size) {
                Err(e) => {
                    let (line, col) = e.line_col(latex);
                    format!("{} (line {}, column {})", e.message, line + 1, col + 1)
                }
                Ok(_) => "the formula could not be laid out".to_owned(),
            };
            Err(Refusal::custom(
                "invalid_latex",
                s.cmd(),
                format!(
                    "The LaTeX could not be parsed: {why}. Fix the expression. Examples: `x^2`, `\\frac{{a}}{{b}}`, `\\sqrt{{x}}`, `\\sum_{{i=1}}^n i`."
                ),
            ))
        }
    }
}

/// The displayed zoom (1 is 100%), which the binding reach and the elbow router are written for.
fn displayed_zoom(s: &Session) -> f64 {
    s.view().zoom / BASE_ZOOM
}

// ---------------------------------------------------------------------------------------------
// element.create
// ---------------------------------------------------------------------------------------------

/// Arguments of `element.create`. Which fields apply depends on `kind`; a field that does not apply to the kind
/// is refused by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateArgs {
    /// What to create: rectangle, diamond, ellipse, frame, line, arrow, freehand, text or math. Images are made
    /// with `image.insert`.
    pub kind: Kind,
    /// rectangle, diamond, ellipse, frame: the box. A negative w or h is normalised. Both sides must be at least 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<RectArg>,
    /// line, arrow, freehand: points in world coordinates. A line or arrow needs 2 or more; a stroke needs 1 or
    /// more (one point is a dot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<Pt>>,
    /// line, arrow: sharp (straight segments, default), round (a curve through the points) or elbow (orthogonal
    /// route between the two ends).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_type: Option<PathType>,
    /// line: the last point connects back to the first (needs 3 or more points).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<bool>,
    /// line, arrow: bind the first point to an element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<BindArg>,
    /// line, arrow: bind the last point to an element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<BindArg>,
    /// line, arrow: automatic binding of ends that are not named in `start` or `end`. `auto` (default) binds an
    /// end that lies inside a bindable element or near its outline; `off` binds nothing; `inside` binds an end over
    /// an element inside it; `on` is `auto` that ignores the setting. The setting `arrow.binding` set to false makes
    /// `auto` act as `off`. Only `auto` follows the setting: `on`, `off` and `inside` are always obeyed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<BindPolicy>,
    /// line, arrow: marker at the first point. Default none. `null` or `"none"` means no marker.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Head>")]
    pub start_head: Option<Option<Head>>,
    /// line, arrow: marker at the last point. Default `arrow` for an arrow and none for a line.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Head>")]
    pub end_head: Option<Option<Head>>,
    /// freehand: pen pressure per point, 0 to 1, one per point. Without it the width is uniform (or simulated by
    /// `style.pressure_mode`). With it the stroke uses device pressure unless the style says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressures: Option<Vec<f64>>,
    /// freehand: a marker stroke, at least 12 wide with alpha 128 unless the style gives a width or colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlighter: Option<bool>,
    /// freehand: tolerance of the simplification in world units, 0 keeps all points. Default 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simplify: Option<f64>,
    /// text: the content. Blank text is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// text: top-left corner of the box. math: top-left corner of the formula's box. Not with `container`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Pt>,
    /// text: fixed width in world units; the text wraps and the box keeps the width. Absent: the box follows the
    /// content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// text: create the text as the label of this rectangle, diamond, ellipse or arrow (an id or `@name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<Ref>,
    /// math: the LaTeX source. Default `x^2 + y^2 = r^2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latex: Option<String>,
    /// Style properties over the style defaults of the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<StyleProps>,
    /// A unique name, so that agents can address the element as `@name`. 1 to 64 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The id of the new element. Default: a fresh id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Select the new element (default true).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub select: bool,
    /// `"auto"` (default): join the frame under the element's centre. An id or `@name`: join that frame. `null`:
    /// no frame. A label joins the frame of its container whatever this says.
    #[serde(default, skip_serializing_if = "FrameArg::is_auto")]
    pub frame: FrameArg,
    /// frame: take in every unlocked element that lies completely inside (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<bool>,
    /// Put the new element directly above this element in the z-order (default: on top, or just below the frame
    /// for a frame child; then `after` must be a child of the same frame). Not with `container`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Ref>,
    /// Passed by the drawing tools: afterwards the tool returns to select unless the tool is locked.
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_tool: bool,
}

/// What `element.create` made.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CreateOut {
    /// The id of the new element (for a label, the label's id).
    pub id: String,
    /// Its kind.
    pub kind: Kind,
    /// Its box.
    pub bounds: RectArg,
    /// For a label: the id of its container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// The frame it joined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// For a frame: the elements it took in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub captured: Vec<String>,
    /// For a line or arrow: the element its first point is bound to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_target: Option<String>,
    /// For a line or arrow: the element its last point is bound to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_target: Option<String>,
}

const BOX_KINDS: &[Kind] = &[Kind::Rectangle, Kind::Diamond, Kind::Ellipse, Kind::Frame];
const LINEAR_KINDS: &[Kind] = &[Kind::Line, Kind::Arrow];
const POINT_KINDS: &[Kind] = &[Kind::Line, Kind::Arrow, Kind::Freehand];
const FREEHAND: &[Kind] = &[Kind::Freehand];
const TEXT: &[Kind] = &[Kind::Text];
const TEXT_MATH: &[Kind] = &[Kind::Text, Kind::Math];
const MATH: &[Kind] = &[Kind::Math];
const LINE: &[Kind] = &[Kind::Line];
const FRAME: &[Kind] = &[Kind::Frame];

fn kind_list(kinds: &[Kind]) -> String {
    let names: Vec<&str> = kinds.iter().map(|k| k.name()).collect();
    match names.as_slice() {
        [] => String::new(),
        [a] => (*a).to_owned(),
        [init @ .., last] => format!("{} and {}", init.join(", "), last),
    }
}

/// Refuses a field that does not apply to the kind, and a required field that is missing.
fn check_fields(cmd: &str, a: &CreateArgs) -> Result<(), Refusal> {
    let given: [(&str, bool, &[Kind]); 16] = [
        ("bounds", a.bounds.is_some(), BOX_KINDS),
        ("points", a.points.is_some(), POINT_KINDS),
        ("arrow_type", a.arrow_type.is_some(), LINEAR_KINDS),
        ("closed", a.closed.is_some(), LINE),
        ("start", a.start.is_some(), LINEAR_KINDS),
        ("end", a.end.is_some(), LINEAR_KINDS),
        ("bind", a.bind.is_some(), LINEAR_KINDS),
        ("start_head", a.start_head.is_some(), LINEAR_KINDS),
        ("end_head", a.end_head.is_some(), LINEAR_KINDS),
        ("pressures", a.pressures.is_some(), FREEHAND),
        ("highlighter", a.highlighter.is_some(), FREEHAND),
        ("simplify", a.simplify.is_some(), FREEHAND),
        ("text", a.text.is_some(), TEXT),
        ("at", a.at.is_some(), TEXT_MATH),
        ("width", a.width.is_some(), TEXT),
        ("container", a.container.is_some(), TEXT),
    ];
    for (key, present, kinds) in given {
        if present && !kinds.contains(&a.kind) {
            return Err(Refusal::bad_arg(
                cmd,
                key,
                format!("does not apply to {}. It applies to {}", Article::new(a.kind.name()), kind_list(kinds)),
            ));
        }
    }
    for (key, present, kinds) in [("latex", a.latex.is_some(), MATH), ("capture", a.capture.is_some(), FRAME)] {
        if present && !kinds.contains(&a.kind) {
            return Err(Refusal::bad_arg(
                cmd,
                key,
                format!("does not apply to {}. It applies to {}", Article::new(a.kind.name()), kind_list(kinds)),
            ));
        }
    }
    let missing = |key: &str| Refusal::bad_arg(cmd, key, format!("is required for {}", Article::new(a.kind.name())));
    match a.kind {
        k if BOX_KINDS.contains(&k) && a.bounds.is_none() => Err(missing("bounds")),
        k if POINT_KINDS.contains(&k) && a.points.is_none() => Err(missing("points")),
        Kind::Text if a.text.is_none() => Err(missing("text")),
        Kind::Text if a.container.is_none() && a.at.is_none() => {
            Err(Refusal::bad_arg(cmd, "at", "is required for a text, unless it is the label of a `container`"))
        }
        Kind::Math if a.at.is_none() => Err(missing("at")),
        _ => Ok(()),
    }?;
    if a.container.is_some() {
        for (key, present) in [("at", a.at.is_some()), ("width", a.width.is_some()), ("after", a.after.is_some())] {
            if present {
                return Err(Refusal::bad_arg(
                    cmd,
                    key,
                    "does not apply to a label: its container decides where it goes",
                ));
            }
        }
    }
    Ok(())
}

fn world_point(cmd: &str, key: &str, i: usize, p: Pt) -> Result<Point, Refusal> {
    Ok(Point::new(args::world(cmd, &format!("{key}[{i}][0]"), p[0])?, args::world(cmd, &format!("{key}[{i}][1]"), p[1])?))
}

fn world_points(cmd: &str, key: &str, pts: &[Pt]) -> Result<Vec<Point>, Refusal> {
    if pts.len() > MAX_POINTS {
        return Err(Refusal::limit_exceeded(cmd, pts.len(), "points", MAX_POINTS));
    }
    pts.iter().enumerate().map(|(i, p)| world_point(cmd, key, i, *p)).collect()
}

pub(super) fn resolve_id(s: &Session, cmd: &str, raw: &Option<String>) -> Result<Option<ElementId>, Refusal> {
    let Some(raw) = raw else { return Ok(None) };
    let id = ElementId::new(raw).map_err(|e| Refusal::bad_arg(cmd, "id", e.to_string()))?;
    create::ensure_id_free(s.store(), &id).map_err(|e| refuse(cmd, e))?;
    Ok(Some(id))
}

pub(super) fn check_name(s: &Session, cmd: &str, name: &Option<String>) -> Result<Option<String>, Refusal> {
    let Some(n) = name else { return Ok(None) };
    create::validate_name(n).map_err(|e| refuse(cmd, e))?;
    create::ensure_name_free(s.store(), n, None).map_err(|e| refuse(cmd, e))?;
    Ok(Some(n.clone()))
}

fn check_style(cmd: &str, props: &Option<StyleProps>) -> Result<(), Refusal> {
    let Some(p) = props else { return Ok(()) };
    if p.is_empty() {
        return Ok(());
    }
    match p.validate() {
        Ok(()) => Ok(()),
        Err(StyleError::OutOfRange { prop, expected }) => {
            Err(Refusal::bad_arg(cmd, &format!("style.{}", prop.name()), expected))
        }
        Err(e) => Err(Refusal::bad_arg(cmd, "style", e.to_string())),
    }
}

/// Names of the set style properties that do not apply to `e` (a font size on a rectangle).
fn ignored_props(props: &StyleProps, e: &drafftink_scene::Element) -> Vec<&'static str> {
    props.set_props().into_iter().filter(|p| !applies(e, *p)).map(Prop::name).collect()
}

fn end_binding(s: &Session, b: &Option<BindArg>, cmd: &str, key: &str) -> Result<EndBinding, Refusal> {
    let Some(b) = b else { return Ok(EndBinding::Auto) };
    let target = s.resolve_ref(&b.target)?;
    if let Some(fp) = b.fixed_point {
        args::in_range(cmd, &format!("{key}.fixed_point[0]"), fp[0], -100.0, 100.0)?;
        args::in_range(cmd, &format!("{key}.fixed_point[1]"), fp[1], -100.0, 100.0)?;
    }
    Ok(EndBinding::Target { target, mode: b.mode.unwrap_or(BindMode::Orbit), fixed_point: b.fixed_point })
}

fn head_of(h: Option<Option<Head>>, default: Head) -> Head {
    match h {
        None => default,
        Some(None) => Head::None,
        Some(Some(h)) => h,
    }
}

/// What the closure needs, all validated before the first change.
enum Body {
    Box { rect: (f64, f64, f64, f64), capture: bool },
    Linear { world: Vec<Point>, spec: LinearSpec, start: EndBinding, end: EndBinding, auto: AutoBind },
    Freehand { world: Vec<Point>, pressures: Vec<f64>, mode: PressureMode, highlighter: bool },
    Text { content: String, at: Point, width: Option<f64> },
    Label { container: ElementId, content: String },
    Math { latex: String, at: Point, font_size: f64, size: Size },
}

struct Plan {
    kind: Kind,
    body: Body,
    id: Option<ElementId>,
    name: Option<String>,
    style: Style,
    defaults: StyleDefaults,
    props: StyleProps,
    frame: FrameChoice,
    after: Option<ElementId>,
}

struct Made {
    id: ElementId,
    container: Option<ElementId>,
    joined: Joined,
    captured: Vec<ElementId>,
    bound: create::BoundEnds,
    after_ignored: bool,
    ignored: Vec<&'static str>,
    rect: RectArg,
}

fn plan_create(s: &Session, a: &CreateArgs) -> Result<Plan, Refusal> {
    let cmd = s.cmd();
    if a.kind == Kind::Image {
        return Err(Refusal::custom("wrong_kind", cmd, "`element.create` cannot create images. Use `image.insert`."));
    }
    check_fields(cmd, a)?;
    check_style(cmd, &a.style)?;
    let id = resolve_id(s, cmd, &a.id)?;
    let name = check_name(s, cmd, &a.name)?;
    let props = a.style.clone().unwrap_or_default();
    let mut defaults = s.tool.defaults();
    if a.kind == Kind::Freehand && a.highlighter == Some(true) {
        defaults.style = create::highlighter_style(defaults.style, create::Given::default());
    }
    defaults.apply(&props);
    let style = defaults.style.clone();

    let after = a.after.as_ref().map(|r| s.resolve_ref(r)).transpose()?;
    let frame = match &a.frame {
        FrameArg::Auto => FrameChoice::Auto,
        FrameArg::NoFrame => FrameChoice::None,
        FrameArg::Frame(r) => FrameChoice::Frame(s.resolve_ref(r)?),
    };

    let body = match a.kind {
        Kind::Rectangle | Kind::Diamond | Kind::Ellipse | Kind::Frame => {
            let b = a.bounds.ok_or_else(|| Refusal::bad_arg(cmd, "bounds", "is required"))?;
            let (x, y, w, h) =
                (args::world(cmd, "bounds.x", b.x)?, args::world(cmd, "bounds.y", b.y)?, args::world(cmd, "bounds.w", b.w)?, args::world(cmd, "bounds.h", b.h)?);
            let rect = create::normalized_box(x, y, w, h);
            create::check_box_size(a.kind, rect.2, rect.3).map_err(|e| refuse(cmd, e))?;
            Body::Box { rect, capture: a.capture.unwrap_or(true) }
        }
        Kind::Line | Kind::Arrow => {
            let world = world_points(cmd, "points", a.points.as_deref().unwrap_or_default())?;
            let mut spec = LinearSpec::defaults(a.kind);
            spec.path = a.arrow_type.unwrap_or(PathType::Sharp);
            spec.start_head = head_of(a.start_head, spec.start_head);
            spec.end_head = head_of(a.end_head, spec.end_head);
            spec.closed = a.closed.unwrap_or(false);
            let min = if spec.closed { 3 } else { 2 };
            if world.len() < min {
                return Err(refuse(cmd, CreateError::TooFewPoints { kind: a.kind, min, have: world.len() }));
            }
            if spec.closed && spec.path == PathType::Elbow {
                return Err(Refusal::bad_arg(cmd, "closed", "a closed line cannot be an elbow; use arrow_type sharp or round"));
            }
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in &world {
                (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
            }
            create::check_extent(a.kind, x1 - x0, y1 - y0).map_err(|e| refuse(cmd, e))?;
            let auto = match a.bind.unwrap_or(BindPolicy::Auto) {
                BindPolicy::Off => AutoBind::Off,
                BindPolicy::Inside => AutoBind::Inside,
                BindPolicy::Auto if !s.settings.bool_or("arrow.binding", true) => AutoBind::Off,
                BindPolicy::Auto | BindPolicy::On => AutoBind::On,
            };
            Body::Linear {
                world,
                spec,
                start: end_binding(s, &a.start, cmd, "start")?,
                end: end_binding(s, &a.end, cmd, "end")?,
                auto,
            }
        }
        Kind::Freehand => {
            let world = world_points(cmd, "points", a.points.as_deref().unwrap_or_default())?;
            if world.is_empty() {
                return Err(refuse(cmd, CreateError::TooFewPoints { kind: Kind::Freehand, min: 1, have: 0 }));
            }
            let tolerance = match a.simplify {
                Some(v) => args::in_range(cmd, "simplify", v, 0.0, 1000.0)?,
                None => DEFAULT_SIMPLIFY,
            };
            let raw = a.pressures.clone().unwrap_or_default();
            if !raw.is_empty() && raw.len() != world.len() {
                return Err(Refusal::bad_arg(
                    cmd,
                    "pressures",
                    format!("needs one value per point: {} points and {} pressures", world.len(), raw.len()),
                ));
            }
            for (i, p) in raw.iter().enumerate() {
                args::in_range(cmd, &format!("pressures[{i}]"), *p, 0.0, 1.0)?;
            }
            let (world, pressures) = create::simplify_stroke(&world, &raw, tolerance);
            // Pen pressure is used unless the style or the session says how the width should behave.
            let mode = match props.pressure_mode {
                Some(m) => m,
                None if defaults.pressure_mode != PressureMode::Uniform => defaults.pressure_mode,
                None if !pressures.is_empty() => PressureMode::Device,
                None => PressureMode::Uniform,
            };
            Body::Freehand { world, pressures, mode, highlighter: a.highlighter.unwrap_or(false) }
        }
        Kind::Text => {
            let content = a.text.clone().unwrap_or_default();
            if content.trim().is_empty() {
                return Err(empty_text(cmd));
            }
            if content.chars().count() > MAX_TEXT_CHARS {
                return Err(Refusal::limit_exceeded(cmd, content.chars().count(), "characters", MAX_TEXT_CHARS));
            }
            match &a.container {
                Some(c) => Body::Label { container: s.resolve_ref(c)?, content },
                None => {
                    let at = a.at.ok_or_else(|| Refusal::bad_arg(cmd, "at", "is required"))?;
                    let width = a.width.map(|w| args::positive(cmd, "width", w, args::MAX_WORLD)).transpose()?;
                    Body::Text { content, at: world_point(cmd, "at", 0, at).map_err(|_| Refusal::bad_arg(cmd, "at", "must be two finite numbers"))?, width }
                }
            }
        }
        Kind::Math => {
            let latex = a.latex.clone().unwrap_or_else(|| DEFAULT_MATH.to_owned());
            if latex.trim().is_empty() {
                return Err(Refusal::bad_arg(cmd, "latex", format!("must not be blank. Omit it for the placeholder `{DEFAULT_MATH}`")));
            }
            if latex.chars().count() > MAX_LATEX_CHARS {
                return Err(Refusal::limit_exceeded(cmd, latex.chars().count(), "characters", MAX_LATEX_CHARS));
            }
            let at = a.at.ok_or_else(|| Refusal::bad_arg(cmd, "at", "is required"))?;
            let font_size = defaults.text.size;
            Body::Math {
                latex: latex.clone(),
                at: world_point(cmd, "at", 0, at).map_err(|_| Refusal::bad_arg(cmd, "at", "must be two finite numbers"))?,
                font_size,
                size: math_box(s, &latex, font_size)?,
            }
        }
        Kind::Image => return Err(Refusal::custom("wrong_kind", cmd, "`element.create` cannot create images. Use `image.insert`.")),
    };
    // A clear stroke on a kind that is drawn in it, or on a shape without a fill, would create nothing to see.
    let closed = a.kind == Kind::Line && a.closed == Some(true);
    style::check_shows(a.kind, closed, &style).map_err(|e| super::style::invisible(cmd, &e))?;
    Ok(Plan { kind: a.kind, body, id, name, style, defaults, props, frame, after })
}

fn create_element(s: &mut Session, a: CreateArgs) -> Result<CreateOut, Refusal> {
    let cmd = s.cmd();
    let plan = plan_create(s, &a)?;
    let (zoom, midpoint) = (displayed_zoom(s), s.settings.bool_or("arrow.midpoint_snap", false));
    let made = edit_with_measure::<Made, CreateError>(s, |tx, measure, ids| {
        let mut container_of: Option<ElementId> = None;
        let mut captured: Vec<ElementId> = Vec::new();
        let mut bound = create::BoundEnds::default();
        let is_label = matches!(plan.body, Body::Label { .. });
        let wanted = match &plan.id {
            Some(i) => i.clone(),
            None => drafftink_edit::unused_element_id(&*tx, ids),
        };
        let mut style = plan.style.clone();
        style.seed = ids.seed();
        let id = match &plan.body {
            Body::Box { rect, capture } => {
                let id = create::insert_on_top(tx, |z| create::shape_element(wanted.clone(), plan.kind, z, *rect, style))?;
                if plan.kind == Kind::Frame && *capture {
                    captured = frame::capture_contained(tx, &id)?.added;
                }
                id
            }
            Body::Linear { world, spec, start, end, auto } => {
                let id = create::insert_on_top(tx, |z| create::linear_element(wanted.clone(), z, world, spec, style))?;
                bound = create::bind_new_ends(tx, &id, start, end, *auto, spec.path, zoom, midpoint)?;
                id
            }
            Body::Freehand { world, pressures, mode, highlighter } => create::insert_on_top(tx, |z| {
                let spec = StrokeSpec {
                    pressures: pressures.clone(),
                    pressure_mode: *mode,
                    brush: plan.defaults.brush,
                    highlighter: *highlighter,
                };
                create::freehand_element(wanted.clone(), z, world, spec, style)
            })?,
            Body::Text { content, at, width } => create::insert_on_top(tx, |z| {
                let spec = TextSpec { font: plan.defaults.text.clone(), place: TextPlace::TopLeft(*at), width: *width };
                create::text_element(measure, wanted.clone(), z, content, spec, style).map_err(CreateError::from)
            })?,
            Body::Label { container, content } => {
                let mut preset = PresetId::new(Some(wanted.clone()), ids);
                let label = text_bind::create_label(tx, measure, &mut preset, container, content, Some(&plan.defaults.text))?.label;
                // The label takes its colour and opacity from the container; the style argument wins.
                let (color, opacity) = (plan.props.stroke_color, plan.props.opacity);
                if color.is_some() || opacity.is_some() {
                    tx.update(&label, |e| {
                        if let Some(c) = color {
                            e.style.stroke_color = c.0;
                        }
                        if let Some(o) = opacity {
                            e.style.opacity = o;
                        }
                    })?;
                }
                container_of = Some(container.clone());
                label
            }
            Body::Math { latex, at, font_size, size } => {
                create::insert_on_top(tx, |z| Ok(create::math_element(wanted.clone(), z, latex, *font_size, *at, *size, style)))?
            }
        };
        if let Some(n) = &plan.name {
            create::set_name(tx, &id, Some(n))?;
        }
        let joined = if is_label { Joined::default() } else { create::join_frame(tx, &id, &plan.frame)? };
        let mut after_ignored = false;
        if let Some(after) = &plan.after {
            let same_frame = tx.get(after).is_some_and(|e| e.frame_id == joined.frame);
            if same_frame {
                create::place_above(tx, &id, after)?;
            } else {
                after_ignored = true;
            }
        }
        let rect = tx.get(&id).map(rect_out).unwrap_or(RectArg { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });
        let ignored = tx.get(&id).map(|e| ignored_props(&plan.props, e)).unwrap_or_default();
        Ok(Made { id, container: container_of, joined, captured, bound, after_ignored, ignored, rect })
    })
    .map_err(|e| refuse(cmd, e))?;

    if !made.ignored.is_empty() {
        s.warn(format!(
            "These style properties do not apply to {} and were ignored: {}.",
            Article::new(a.kind.name()),
            made.ignored.join(", ")
        ));
    }
    if let Some(f) = &made.joined.left_out {
        s.warn(format!(
            "The new element was not added to frame `{f}`: an arrow joins a frame only when an end is bound to, or lies inside, a member of the frame."
        ));
    }
    if made.after_ignored {
        s.warn("`after` was ignored: the new element is in a different frame than that element, and a frame's children stay together.");
    }
    if a.select {
        select_exactly(s, std::slice::from_ref(&made.id), false);
    }
    if a.from_tool && !s.tool.lock {
        s.tool.active = Tool::Select;
    }
    Ok(CreateOut {
        id: made.id.as_str().to_owned(),
        kind: a.kind,
        bounds: made.rect,
        container: made.container.map(|c| c.as_str().to_owned()),
        frame: made.joined.frame.map(|f| f.as_str().to_owned()),
        captured: id_strings(made.captured.iter()),
        start_target: made.bound.start.map(|i| i.as_str().to_owned()),
        end_target: made.bound.end.map(|i| i.as_str().to_owned()),
    })
}

// ---------------------------------------------------------------------------------------------
// element.connect
// ---------------------------------------------------------------------------------------------

/// What `element.connect` draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectKind {
    /// An arrow (default).
    Arrow,
    /// A line.
    Line,
}

/// Arguments of `element.connect`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectArgs {
    /// The element the arrow starts at: an id or `@name`.
    pub from: Ref,
    /// The element the arrow ends at: an id or `@name`.
    pub to: Ref,
    /// arrow (default) or line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ConnectKind>,
    /// sharp (default), round or elbow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_type: Option<PathType>,
    /// orbit (default): the ends sit on the outlines plus a gap. inside: the ends stay at the facing outline points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_mode: Option<BindMode>,
    /// Text of the arrow's label. Blank or absent: no label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Marker at the start. Default none.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Head>")]
    pub start_head: Option<Option<Head>>,
    /// Marker at the end. Default `arrow` for an arrow, none for a line.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Head>")]
    pub end_head: Option<Option<Head>>,
    /// Style properties over the style defaults of the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<StyleProps>,
    /// A unique name for the arrow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// What `element.connect` made.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ConnectOut {
    /// The id of the new arrow.
    pub id: String,
    /// The id of its label, when `label` was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn connect(s: &mut Session, a: ConnectArgs) -> Result<ConnectOut, Refusal> {
    let cmd = s.cmd();
    check_style(cmd, &a.style)?;
    let (from, to) = (s.resolve_ref(&a.from)?, s.resolve_ref(&a.to)?);
    let name = check_name(s, cmd, &a.name)?;
    let label = a.label.as_deref().filter(|l| !l.trim().is_empty()).map(str::to_owned);
    if label.is_some() && a.kind == Some(ConnectKind::Line) {
        return Err(Refusal::bad_arg(cmd, "label", "only an arrow can have a label. Use kind \"arrow\", or omit the label"));
    }
    if let Some(l) = &label
        && l.chars().count() > MAX_TEXT_CHARS
    {
        return Err(Refusal::limit_exceeded(cmd, l.chars().count(), "characters", MAX_TEXT_CHARS));
    }
    let props = a.style.clone().unwrap_or_default();
    let mut defaults = s.tool.defaults();
    defaults.apply(&props);
    let kind = match a.kind.unwrap_or(ConnectKind::Arrow) {
        ConnectKind::Arrow => Kind::Arrow,
        ConnectKind::Line => Kind::Line,
    };
    let mut linear = LinearSpec::defaults(kind);
    linear.path = a.arrow_type.unwrap_or(PathType::Sharp);
    linear.start_head = head_of(a.start_head, linear.start_head);
    linear.end_head = head_of(a.end_head, linear.end_head);
    let spec = ConnectSpec {
        linear,
        mode: a.bind_mode.unwrap_or(BindMode::Orbit),
        style: defaults.style.clone(),
        zoom: displayed_zoom(s),
        midpoint_snap: s.settings.bool_or("arrow.midpoint_snap", false),
    };
    let (arrow, label_id, ignored) = edit_with_measure::<(ElementId, Option<ElementId>, Vec<&'static str>), CreateError>(s, |tx, measure, ids| {
        let arrow = create::connect(tx, ids, &from, &to, &spec)?;
        if let Some(n) = &name {
            create::set_name(tx, &arrow, Some(n))?;
        }
        let label_id = match &label {
            Some(text) => Some(text_bind::create_label(tx, measure, ids, &arrow, text, Some(&defaults.text))?.label),
            None => None,
        };
        let ignored = tx.get(&arrow).map(|e| ignored_props(&props, e)).unwrap_or_default();
        Ok((arrow, label_id, ignored))
    })
    .map_err(|e| refuse(cmd, e))?;
    if !ignored.is_empty() {
        s.warn(format!("These style properties do not apply to {} and were ignored: {}.", Article::new(kind.name()), ignored.join(", ")));
    }
    select_exactly(s, std::slice::from_ref(&arrow), false);
    Ok(ConnectOut { id: arrow.as_str().to_owned(), label: label_id.map(|l| l.as_str().to_owned()) })
}

// ---------------------------------------------------------------------------------------------
// element.relabel
// ---------------------------------------------------------------------------------------------

/// Arguments of `element.relabel`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelabelArgs {
    /// A text element, or a rectangle, diamond, ellipse or arrow with or without a label: an id or `@name`.
    pub id: Ref,
    /// The new text. Empty text removes the label of a shape or arrow, or a bound label given as `id`. A
    /// free text element cannot be empty (`empty_text`).
    pub text: String,
    /// Create a label when the shape or arrow has none (default true).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub create: bool,
}

/// What `element.relabel` did.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RelabelOut {
    /// The element that was given.
    pub id: String,
    /// `updated`, `created`, `removed` or `none` (there was no label and none was asked for).
    pub action: &'static str,
    /// The label element after the change (absent after `removed` or `none`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn relabel(s: &mut Session, a: RelabelArgs) -> Result<RelabelOut, Refusal> {
    let cmd = s.cmd();
    let id = s.resolve_ref(&a.id)?;
    let kind = s.element(&id).map(|e| e.tag()).ok_or_else(|| Refusal::not_found(cmd, &a.id.0))?;
    if kind != Kind::Text && !kind.is_label_container() {
        return Err(Refusal::wrong_kind(cmd, kind.name(), "text, rectangle, diamond, ellipse and arrow"));
    }
    if a.text.chars().count() > MAX_TEXT_CHARS {
        return Err(Refusal::limit_exceeded(cmd, a.text.chars().count(), "characters", MAX_TEXT_CHARS));
    }
    let defaults = s.tool.text_defaults.clone();
    let outcome = edit_with_measure::<Relabeled, CreateError>(s, |tx, measure, ids| {
        text_bind::relabel(tx, measure, ids, &id, &a.text, a.create, Some(&defaults)).map_err(CreateError::from)
    })
    .map_err(|e| refuse(cmd, e))?;
    let (action, label) = match outcome {
        Relabeled::Updated(o) => ("updated", Some(o.label)),
        Relabeled::Created(o) => ("created", Some(o.label)),
        Relabeled::Removed(_) => ("removed", None),
        Relabeled::Nothing => ("none", None),
    };
    prune_selection(s);
    Ok(RelabelOut { id: id.as_str().to_owned(), action, label: label.map(|l| l.as_str().to_owned()) })
}

// ---------------------------------------------------------------------------------------------
// element.set_name
// ---------------------------------------------------------------------------------------------

/// Arguments of `element.set_name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetNameArgs {
    /// The element: an id or `@name`.
    pub id: Ref,
    /// The new name (1 to 64 characters, not starting with `@` or `$`). `null` or absent clears the name.
    #[serde(default)]
    pub name: Option<String>,
}

/// The name after `element.set_name`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SetNameOut {
    /// The element.
    pub id: String,
    /// Its name now; absent when it has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn set_name(s: &mut Session, a: SetNameArgs) -> Result<SetNameOut, Refusal> {
    let cmd = s.cmd();
    let id = s.resolve_ref(&a.id)?;
    s.tab_mut().doc.edit::<bool, CreateError>(|tx| create::set_name(tx, &id, a.name.as_deref())).map_err(|e| refuse(cmd, e))?;
    Ok(SetNameOut { id: id.as_str().to_owned(), name: a.name })
}

// ---------------------------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------------------------

const DOC_OBSERVED: Facts = Facts {
    undo: Undo::Undoable,
    reads_only: false,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    needs_selection: Selection::No,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Scene,
    ring: RingMode::Always,
    palette: false,
    agent: true,
    doc_write: crate::spec::DocWrite::Edit,
};

const CREATE: Facts = Facts { mutates: Mutates::DOC.with(Mutates::OBSERVED).with(Mutates::TOOL), ..DOC_OBSERVED };

/// Commands of this part of the family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "element.create",
        "element",
        "Create element",
        "Creates one element of any kind except image (shape, frame, line, arrow, stroke, text with an optional container, or math) and selects it.",
        &Typed::new(create_element),
        CREATE,
    )
    .refusals(&[
        "invisible",
        "too_small",
        "empty_text",
        "id_in_use",
        "name_taken",
        "bad_name",
        "not_bindable",
        "already_bound",
        "invalid_latex",
    ])
    .example(r#"{"kind":"rectangle","bounds":{"x":0,"y":0,"w":120,"h":60},"name":"box"}"#),
    CommandSpec::new(
        "element.set_name",
        "element",
        "Name element",
        "Sets or clears the unique name of an element, so that it can be addressed as `@name`.",
        &Typed::new(set_name),
        DOC_OBSERVED,
    )
    .refusals(&["name_taken", "bad_name"])
    .example(r#"{"id":"e1","name":"box"}"#),
    CommandSpec::new(
        "element.relabel",
        "element",
        "Change label",
        "Replaces the text of a text element, or the label of a shape or arrow; creates the label when there is none, removes it for empty text.",
        &Typed::new(relabel),
        DOC_OBSERVED,
    )
    .refusals(&["empty_text"])
    .example(r#"{"id":"e1","text":"Start"}"#),
    CommandSpec::new(
        "element.connect",
        "element",
        "Connect elements",
        "Creates an arrow (or line) bound to two elements at the sides that face each other, with an optional label.",
        &Typed::new(connect),
        DOC_OBSERVED,
    )
    .refusals(&["same_element", "not_bindable", "name_taken", "bad_name"])
    .example(r#"{"from":"e1","to":"e2","label":"yes"}"#),
];

#[cfg(test)]
#[path = "element_create_tests.rs"]
mod tests;
