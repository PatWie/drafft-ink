//! `view`: camera, view-only mode and screenshots (docs/commands.md §2.14).
//!
//! The worked example of a family; `docs/engine.md` walks through it. The camera is a scroll point (the
//! world point at the centre of the viewport) and a zoom, clamped to 0.1..10. Per-frame camera movement
//! by pointer or wheel is a documented exception (docs/exceptions.md E10, E11): the shell moves its own
//! viewport and reports the final camera with `view.set`. The commands here are the scriptable
//! equivalents and the final-state commands.

use base64::Engine as _;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_geom::{OrientedBox, aabb};
use drafftink_scene::{Element, ElementId, SceneRead};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::args;
use crate::ports::{PortKind, RenderRequest};
use crate::refusal::Refusal;
use crate::session::{Ref, Session};
use crate::spec::{Binding, CommandSpec, Facts, NoArgs, Scope, Typed};
use crate::state::{BASE_ZOOM, View, ZOOM_MAX, ZOOM_MIN, clamp_zoom};

/// A point in world units or screen pixels: `[x, y]`.
pub type Pt = [f64; 2];

/// A world rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RectArg {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// Where a zoom is anchored.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum At {
    /// The word `"viewport_center"`.
    Keyword(CenterWord),
    /// A screen point `[x, y]` in logical pixels from the top-left of the canvas.
    Point(Pt),
}

/// The only word `at` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CenterWord {
    /// The middle of the viewport.
    ViewportCenter,
}

impl Default for At {
    fn default() -> Self {
        At::Keyword(CenterWord::ViewportCenter)
    }
}

impl At {
    fn screen(self, v: &View) -> Point {
        match self {
            At::Keyword(_) => Point::new(v.viewport.w / 2.0, v.viewport.h / 2.0),
            At::Point(p) => Point::new(p[0], p[1]),
        }
    }
}

/// The camera after a view command.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CameraOut {
    /// False when the camera was already as requested.
    pub changed: bool,
    /// The world point at the centre of the viewport.
    pub scroll: Pt,
    /// Screen pixels per world unit.
    pub zoom: f64,
    /// The zoom as the UI shows it.
    pub percent: f64,
}

fn camera_out(before: &View, after: &View) -> CameraOut {
    CameraOut {
        changed: before != after,
        scroll: [after.scroll.x, after.scroll.y],
        zoom: after.zoom,
        percent: after.percent(),
    }
}

/// Edits the active view with `f` and reports the result.
fn with_view(s: &mut Session, f: impl FnOnce(&mut View) -> Result<(), Refusal>) -> Result<CameraOut, Refusal> {
    let before = *s.view();
    let mut after = before;
    f(&mut after)?;
    s.tab_mut().view = after;
    Ok(camera_out(&before, &after))
}

fn pt(command: &str, key: &str, p: Pt) -> Result<Point, Refusal> {
    Ok(Point::new(args::world(command, &format!("{key}[0]"), p[0])?, args::world(command, &format!("{key}[1]"), p[1])?))
}

/// Zooms to `zoom` keeping the world point under `anchor` (screen) fixed.
fn zoom_about(v: &mut View, zoom: f64, anchor: Point) {
    let world = v.screen_to_world(anchor);
    v.zoom = clamp_zoom(zoom);
    v.scroll = Point::new(world.x - (anchor.x - v.viewport.w / 2.0) / v.zoom, world.y - (anchor.y - v.viewport.h / 2.0) / v.zoom);
}

// ---- view.set ----

/// Arguments of `view.set`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SetArgs {
    /// The world point at the centre of the viewport.
    pub scroll: Option<Pt>,
    /// Screen pixels per world unit, clamped to 0.1..10.
    pub zoom: Option<f64>,
}

fn set(s: &mut Session, a: SetArgs) -> Result<CameraOut, Refusal> {
    let id = s.cmd();
    let scroll = a.scroll.map(|p| pt(id, "scroll", p)).transpose()?;
    if let Some(z) = a.zoom
        && !z.is_finite()
    {
        return Err(Refusal::bad_arg(id, "zoom", "must be a finite number"));
    }
    with_view(s, |v| {
        if let Some(p) = scroll {
            v.scroll = p;
        }
        if let Some(z) = a.zoom {
            v.zoom = clamp_zoom(z);
        }
        Ok(())
    })
}

// ---- view.pan ----

/// Arguments of `view.pan`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PanArgs {
    /// Screen pixels the content moves to the right (as the pointer would).
    pub dx: f64,
    /// Screen pixels the content moves down.
    pub dy: f64,
}

fn pan(s: &mut Session, a: PanArgs) -> Result<CameraOut, Refusal> {
    let id = s.cmd();
    let (dx, dy) = (args::world(id, "dx", a.dx)?, args::world(id, "dy", a.dy)?);
    with_view(s, |v| {
        v.scroll = Point::new(v.scroll.x - dx / v.zoom, v.scroll.y - dy / v.zoom);
        Ok(())
    })
}

// ---- view.zoom_at / zoom_in / zoom_out ----

/// Arguments of `view.zoom_at`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomAtArgs {
    /// Multiply the zoom by this. Give exactly one of `factor` and `zoom`.
    pub factor: Option<f64>,
    /// Set the zoom to this.
    pub zoom: Option<f64>,
    /// The screen point that stays fixed. Default: the viewport centre.
    pub at: At,
}

fn zoom_at(s: &mut Session, a: ZoomAtArgs) -> Result<CameraOut, Refusal> {
    let id = s.cmd();
    let target = match (a.factor, a.zoom) {
        (Some(f), None) => {
            let f = args::positive(id, "factor", f, 1.0e6)?;
            Target::Factor(f)
        }
        (None, Some(z)) => Target::Zoom(args::positive(id, "zoom", z, 1.0e6)?),
        _ => return Err(Refusal::bad_args(id, "Give exactly one of `factor` or `zoom`")),
    };
    let anchor_arg = match a.at {
        At::Point(p) => At::Point([args::world(id, "at[0]", p[0])?, args::world(id, "at[1]", p[1])?]),
        k => k,
    };
    with_view(s, |v| {
        let z = match target {
            Target::Factor(f) => v.zoom * f,
            Target::Zoom(z) => z,
        };
        let anchor = anchor_arg.screen(v);
        zoom_about(v, z, anchor);
        Ok(())
    })
}

enum Target {
    Factor(f64),
    Zoom(f64),
}

/// Arguments of `view.zoom_in` and `view.zoom_out`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomStepArgs {
    /// The screen point that stays fixed. Default: the viewport centre.
    pub at: At,
}

fn zoom_step(s: &mut Session, at: At, factor: f64) -> Result<CameraOut, Refusal> {
    let id = s.cmd();
    let at = match at {
        At::Point(p) => At::Point([args::world(id, "at[0]", p[0])?, args::world(id, "at[1]", p[1])?]),
        k => k,
    };
    with_view(s, |v| {
        let anchor = at.screen(v);
        zoom_about(v, v.zoom * factor, anchor);
        Ok(())
    })
}

fn zoom_in(s: &mut Session, a: ZoomStepArgs) -> Result<CameraOut, Refusal> {
    zoom_step(s, a.at, 1.25)
}

fn zoom_out(s: &mut Session, a: ZoomStepArgs) -> Result<CameraOut, Refusal> {
    zoom_step(s, a.at, 0.8)
}

// ---- bounds helpers ----

/// The world box of an element: its rotated box. Lines, arrows and freehand use the box of their points
/// (that is what the transform holds).
fn element_bounds(e: &Element) -> Rect {
    let t = &e.transform;
    OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle).aabb()
}

fn bounds_of(s: &Session, ids: impl IntoIterator<Item = ElementId>) -> Option<Rect> {
    aabb::union_all(ids.into_iter().filter_map(|id| s.element(&id)).map(element_bounds))
}

fn content_bounds(s: &Session) -> Option<Rect> {
    aabb::union_all(s.store().iter().map(element_bounds))
}

fn selection_bounds(s: &Session) -> Option<Rect> {
    bounds_of(s, s.selection().iter().cloned())
}

// ---- view.zoom_reset ----

/// Arguments of `view.zoom_reset`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomResetArgs {
    /// Also move the centre of the content to the centre of the viewport.
    pub recenter: bool,
}

fn zoom_reset(s: &mut Session, a: ZoomResetArgs) -> Result<CameraOut, Refusal> {
    let content = if a.recenter { content_bounds(s) } else { None };
    if a.recenter && content.is_none() {
        s.warn("The document is empty, so the view was not recentred.");
    }
    with_view(s, |v| {
        v.zoom = BASE_ZOOM;
        if let Some(r) = content {
            v.scroll = aabb::center(&r);
        }
        Ok(())
    })
}

// ---- view.zoom_to_fit ----

/// What `view.zoom_to_fit` fits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FitScope {
    /// The selection if there is one, else everything.
    #[default]
    Auto,
    /// All content.
    All,
    /// The selection.
    Selection,
    /// The elements in `ids`.
    Ids,
    /// The rectangle in `rect`.
    Rect,
}

/// Arguments of `view.zoom_to_fit`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct FitArgs {
    /// What to fit.
    pub scope: FitScope,
    /// The elements, for `scope: ids`.
    pub ids: Option<Vec<Ref>>,
    /// The rectangle, for `scope: rect`.
    pub rect: Option<RectArg>,
    /// Screen pixels kept free around the content.
    pub padding: f64,
    /// The largest zoom to use, so that one small element is not blown up.
    pub max_zoom: f64,
}

impl Default for FitArgs {
    fn default() -> Self {
        Self { scope: FitScope::Auto, ids: None, rect: None, padding: 50.0, max_zoom: 3.36 }
    }
}

fn zoom_to_fit(s: &mut Session, a: FitArgs) -> Result<CameraOut, Refusal> {
    let id = s.cmd();
    let padding = args::in_range(id, "padding", a.padding, 0.0, 10_000.0)?;
    let max_zoom = args::in_range(id, "max_zoom", a.max_zoom, ZOOM_MIN, ZOOM_MAX)?;
    let scope = match a.scope {
        FitScope::Auto if s.selection().is_empty() => FitScope::All,
        FitScope::Auto => FitScope::Selection,
        other => other,
    };
    let rect = match scope {
        FitScope::All | FitScope::Auto => content_bounds(s).ok_or_else(|| {
            Refusal::nothing_to_do(id, "fit", "the document is empty. Draw something, or use `view.center`")
        })?,
        FitScope::Selection => selection_bounds(s).ok_or_else(|| {
            Refusal::nothing_to_do(id, "fit", "nothing is selected. Select something, or use scope=all")
        })?,
        FitScope::Ids => {
            let refs = a.ids.as_deref().ok_or_else(|| Refusal::bad_arg(id, "ids", "is needed when scope is `ids`"))?;
            let ids = s.targets(Some(refs))?;
            bounds_of(s, ids).ok_or_else(|| Refusal::nothing_to_do(id, "fit", "the elements have no size"))?
        }
        FitScope::Rect => {
            let r = a.rect.ok_or_else(|| Refusal::bad_arg(id, "rect", "is needed when scope is `rect`"))?;
            let (x, y) = (args::world(id, "rect.x", r.x)?, args::world(id, "rect.y", r.y)?);
            let (w, h) = (args::world(id, "rect.w", r.w)?, args::world(id, "rect.h", r.h)?);
            if w < 0.0 || h < 0.0 {
                return Err(Refusal::bad_arg(id, "rect", "width and height cannot be negative"));
            }
            Rect::new(x, y, x + w, y + h)
        }
    };
    with_view(s, |v| {
        let avail_w = (v.viewport.w - 2.0 * padding).max(1.0);
        let avail_h = (v.viewport.h - 2.0 * padding).max(1.0);
        let fit_w = if rect.width() > 0.0 { avail_w / rect.width() } else { f64::INFINITY };
        let fit_h = if rect.height() > 0.0 { avail_h / rect.height() } else { f64::INFINITY };
        let zoom = fit_w.min(fit_h).min(max_zoom);
        v.zoom = clamp_zoom(if zoom.is_finite() { zoom } else { max_zoom });
        v.scroll = aabb::center(&rect);
        Ok(())
    })
}

// ---- view.center ----

/// What `view.center` centres on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CenterOn {
    /// The world origin.
    #[default]
    Origin,
    /// The middle of all content.
    Content,
    /// The middle of the selection.
    Selection,
    /// The world point in `point`.
    Point,
}

/// Arguments of `view.center`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CenterArgs {
    /// What to centre on.
    pub on: CenterOn,
    /// The world point, for `on: point`.
    pub point: Option<Pt>,
}

fn center(s: &mut Session, a: CenterArgs) -> Result<CameraOut, Refusal> {
    let id = s.cmd();
    let target = match a.on {
        CenterOn::Origin => Point::ZERO,
        CenterOn::Content => content_bounds(s)
            .map(|r| aabb::center(&r))
            .ok_or_else(|| Refusal::nothing_to_do(id, "centre", "the document is empty. Draw something, or use on=origin"))?,
        CenterOn::Selection => selection_bounds(s)
            .map(|r| aabb::center(&r))
            .ok_or_else(|| Refusal::nothing_to_do(id, "centre", "nothing is selected. Select something, or use on=content"))?,
        CenterOn::Point => pt(id, "point", a.point.ok_or_else(|| Refusal::bad_arg(id, "point", "is needed when on is `point`"))?)?,
    };
    with_view(s, |v| {
        v.scroll = target;
        Ok(())
    })
}

// ---- view.scroll_page ----

/// A direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    /// Towards the top of the document.
    Up,
    /// Towards the bottom.
    Down,
    /// Towards the left.
    Left,
    /// Towards the right.
    Right,
}

/// Arguments of `view.scroll_page`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollPageArgs {
    /// Which way to scroll.
    pub direction: Dir,
    /// How much of the viewport to scroll, above 0 and at most 10. Default 0.8.
    #[serde(default = "default_fraction")]
    pub fraction: f64,
}

fn default_fraction() -> f64 {
    0.8
}

fn scroll_page(s: &mut Session, a: ScrollPageArgs) -> Result<CameraOut, Refusal> {
    let f = args::positive(s.cmd(), "fraction", a.fraction, 10.0)?;
    with_view(s, |v| {
        let (dx, dy) = match a.direction {
            Dir::Up => (0.0, -f * v.viewport.h / v.zoom),
            Dir::Down => (0.0, f * v.viewport.h / v.zoom),
            Dir::Left => (-f * v.viewport.w / v.zoom, 0.0),
            Dir::Right => (f * v.viewport.w / v.zoom, 0.0),
        };
        v.scroll = Point::new(v.scroll.x + dx, v.scroll.y + dy);
        Ok(())
    })
}

// ---- view.set_viewport ----

/// Arguments of `view.set_viewport`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetViewportArgs {
    /// Canvas width in logical pixels.
    pub width: f64,
    /// Canvas height in logical pixels.
    pub height: f64,
    /// Device pixels per logical pixel. Default 1.
    #[serde(default = "one")]
    pub scale_factor: f64,
}

fn one() -> f64 {
    1.0
}

/// The viewport after `view.set_viewport`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ViewportOut {
    /// False when the size was already this.
    pub changed: bool,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
    /// Device pixels per logical pixel.
    pub scale_factor: f64,
}

fn set_viewport(s: &mut Session, a: SetViewportArgs) -> Result<ViewportOut, Refusal> {
    let id = s.cmd();
    let (w, h) = (args::positive(id, "width", a.width, 100_000.0)?, args::positive(id, "height", a.height, 100_000.0)?);
    let sf = args::positive(id, "scale_factor", a.scale_factor, 16.0)?;
    let mut changed = false;
    for t in &mut s.tabs {
        if t.view.viewport.w != w || t.view.viewport.h != h || t.view.viewport.scale_factor != sf {
            changed = true;
        }
        t.view.viewport.w = w;
        t.view.viewport.h = h;
        t.view.viewport.scale_factor = sf;
    }
    Ok(ViewportOut { changed, width: w, height: h, scale_factor: sf })
}

// ---- view.set_view_only ----

/// Arguments of `view.set_view_only`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ViewOnlyArgs {
    /// On or off. Omitted: toggle.
    pub on: Option<bool>,
}

/// The mode after `view.set_view_only`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ViewOnlyOut {
    /// False when the mode was already as requested.
    pub changed: bool,
    /// The mode now.
    pub view_only: bool,
}

fn set_view_only(s: &mut Session, a: ViewOnlyArgs) -> Result<ViewOnlyOut, Refusal> {
    let on = a.on.unwrap_or(!s.view_only);
    let changed = s.view_only != on;
    s.view_only = on;
    if on {
        // A running drag could not have started in this mode: end it as if it were cancelled.
        s.cancel_gesture_if_running();
    }
    Ok(ViewOnlyOut { changed, view_only: on })
}

// ---- view.get ----

/// The state `view.get` reports.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ViewInfo {
    /// Always false: reading changes nothing.
    pub changed: bool,
    /// The world point at the centre of the viewport.
    pub scroll: Pt,
    /// Screen pixels per world unit.
    pub zoom: f64,
    /// The zoom as the UI shows it.
    pub percent: f64,
    /// Canvas size in logical pixels.
    pub viewport: ViewportInfo,
    /// The visible world rectangle.
    pub visible: RectArg,
    /// View-only mode.
    pub view_only: bool,
    /// The tool in effect.
    pub tool: String,
    /// Tool lock.
    pub tool_lock: bool,
}

/// Canvas size.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ViewportInfo {
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
    /// Device pixels per logical pixel.
    pub scale_factor: f64,
}

fn get(s: &mut Session, _: NoArgs) -> Result<ViewInfo, Refusal> {
    let v = *s.view();
    let (x, y, w, h) = v.visible();
    Ok(ViewInfo {
        changed: false,
        scroll: [v.scroll.x, v.scroll.y],
        zoom: v.zoom,
        percent: v.percent(),
        viewport: ViewportInfo { w: v.viewport.w, h: v.viewport.h, scale_factor: v.viewport.scale_factor },
        visible: RectArg { x, y, w, h },
        view_only: s.view_only,
        tool: s.tool.effective().name().to_owned(),
        tool_lock: s.tool.lock,
    })
}

// ---- view.screenshot ----

/// What `view.screenshot` draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Region {
    /// `"viewport"`, `"all"` or `"selection"`.
    Named(RegionWord),
    /// A world rectangle.
    Rect(RectArg),
    /// The bounds of these elements.
    Ids {
        /// The elements.
        ids: Vec<Ref>,
    },
}

/// The named regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegionWord {
    /// What the camera shows.
    Viewport,
    /// All content.
    All,
    /// The selection.
    Selection,
}

/// Arguments of `view.screenshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ScreenshotArgs {
    /// What to draw. Default: the viewport.
    pub region: Region,
    /// Output pixels per screen pixel (for the viewport) or per world unit (otherwise), 0.05 to 8.
    pub scale: f64,
    /// The longest side of the picture in pixels, 16 to 8192.
    pub max_side_px: i64,
    /// Draw the document background.
    pub background: bool,
    /// Draw selection handles and guides.
    pub overlay: bool,
}

impl Default for ScreenshotArgs {
    fn default() -> Self {
        Self { region: Region::Named(RegionWord::Viewport), scale: 1.0, max_side_px: 2048, background: true, overlay: false }
    }
}

/// A PNG as base64.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PngBytes {
    /// Base64 of the PNG.
    pub base64: String,
    /// Always `image/png`.
    pub mime: &'static str,
}

/// Output of `view.screenshot`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ScreenshotOut {
    /// Always false: a screenshot changes nothing.
    pub changed: bool,
    /// The picture.
    pub bytes: PngBytes,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

fn screenshot(s: &mut Session, a: ScreenshotArgs) -> Result<ScreenshotOut, Refusal> {
    let id = s.cmd();
    if !s.has_port(PortKind::Renderer) {
        return Err(Refusal::port_unavailable(
            id,
            "the renderer",
            "headless without a GPU",
            "Read the structure with `doc.outline` instead.",
        ));
    }
    let scale = args::in_range(id, "scale", a.scale, 0.05, 8.0)?;
    let max_side = u32::try_from(a.max_side_px)
        .ok()
        .filter(|m| (16..=8192).contains(m))
        .ok_or_else(|| Refusal::bad_arg(id, "max_side_px", "must be a whole number from 16 to 8192"))?;
    let v = *s.view();
    let (region, per_unit) = match &a.region {
        Region::Named(RegionWord::Viewport) => {
            let (x, y, w, h) = v.visible();
            (Rect::new(x, y, x + w, y + h), v.zoom * scale)
        }
        Region::Named(RegionWord::All) => (
            content_bounds(s).ok_or_else(|| Refusal::nothing_to_do(id, "draw", "the document is empty"))?,
            scale,
        ),
        Region::Named(RegionWord::Selection) => (
            selection_bounds(s).ok_or_else(|| Refusal::nothing_to_do(id, "draw", "nothing is selected"))?,
            scale,
        ),
        Region::Rect(r) => {
            let (x, y) = (args::world(id, "region.x", r.x)?, args::world(id, "region.y", r.y)?);
            let (w, h) = (args::world(id, "region.w", r.w)?, args::world(id, "region.h", r.h)?);
            if w <= 0.0 || h <= 0.0 {
                return Err(Refusal::nothing_to_do(id, "draw", "the region has no area"));
            }
            (Rect::new(x, y, x + w, y + h), scale)
        }
        Region::Ids { ids } => {
            let ids = s.targets(Some(ids))?;
            (bounds_of(s, ids).ok_or_else(|| Refusal::nothing_to_do(id, "draw", "the elements have no size"))?, scale)
        }
    };
    let (mut pw, mut ph) = (region.width() * per_unit, region.height() * per_unit);
    if !(pw.is_finite() && ph.is_finite()) || pw < 1.0 && ph < 1.0 {
        return Err(Refusal::nothing_to_do(id, "draw", "the region is too small to draw"));
    }
    let longest = pw.max(ph);
    if longest > f64::from(max_side) {
        let k = f64::from(max_side) / longest;
        pw *= k;
        ph *= k;
    }
    let (width, height) = (pw.round().max(1.0) as u32, ph.round().max(1.0) as u32);
    let req = RenderRequest {
        region: (region.x0, region.y0, region.width(), region.height()),
        size_px: (width, height),
        background: a.background,
        overlay: a.overlay,
        selection: if a.overlay { s.selection().iter().cloned().collect() } else { Vec::new() },
        background_color: s.store().meta().background,
    };
    let scene = s.tab().doc.snapshot();
    let png = match s.ports.renderer.as_ref() {
        Some(r) => r.render_png(&scene, &req).map_err(|e| Refusal::io_failed(id, "render", "the screenshot", &e.to_string()))?,
        None => return Err(Refusal::internal(id, "the renderer disappeared")),
    };
    Ok(ScreenshotOut { changed: false, bytes: PngBytes { base64: base64::engine::general_purpose::STANDARD.encode(&png), mime: "image/png" }, width, height })
}

// ---- registration ----

const ZOOM_IN_KEYS: &[Binding] = &[Binding::new(&["Mod+=", "Mod++"], Scope::Canvas)];
const ZOOM_OUT_KEYS: &[Binding] = &[Binding::new(&["Mod+-"], Scope::Canvas)];
const ZOOM_RESET_KEYS: &[Binding] = &[Binding::new(&["Mod+0"], Scope::Canvas)];
const FIT_KEYS: &[Binding] = &[
    Binding::new(&["Shift+1"], Scope::Canvas).with_args(r#"{"scope":"all"}"#).titled("Zoom to fit everything"),
    Binding::new(&["Shift+2", "Shift+3"], Scope::Canvas).with_args(r#"{"scope":"selection"}"#).titled("Zoom to selection"),
];
const SCROLL_KEYS: &[Binding] = &[
    Binding::new(&["PgUp"], Scope::Canvas).with_args(r#"{"direction":"up"}"#),
    Binding::new(&["PgDn"], Scope::Canvas).with_args(r#"{"direction":"down"}"#),
    Binding::new(&["Shift+PgUp"], Scope::Canvas).with_args(r#"{"direction":"left"}"#),
    Binding::new(&["Shift+PgDn"], Scope::Canvas).with_args(r#"{"direction":"right"}"#),
];
const VIEW_ONLY_KEYS: &[Binding] = &[Binding::new(&["Alt+R"], Scope::Always)];

/// `view.set`, `view.pan`, `view.zoom_at` and `view.set_viewport` fire many times a second.
const QUIET: Facts = Facts { ring: crate::spec::RingMode::Quiet, ..Facts::CAMERA };
/// A camera command with a palette entry.
const PALETTE: Facts = Facts { palette: true, ..Facts::CAMERA };

/// The view family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "view.set",
        "view",
        "Set camera",
        "Sets the camera: the world point at the centre of the viewport, and the zoom (clamped to 0.1..10).",
        &Typed::new(set),
        QUIET,
    )
    .example(r#"{"scroll":[200,100],"zoom":1.68}"#),
    CommandSpec::new(
        "view.pan",
        "view",
        "Pan",
        "Pans the camera by screen pixels; the content moves by the same amount as the pointer would.",
        &Typed::new(pan),
        QUIET,
    )
    .example(r#"{"dx":-120,"dy":0}"#),
    CommandSpec::new(
        "view.zoom_at",
        "view",
        "Zoom at point",
        "Zooms by a factor or to a zoom value, keeping the point under `at` fixed.",
        &Typed::new(zoom_at),
        QUIET,
    )
    .example(r#"{"factor":1.1,"at":[400,300]}"#),
    CommandSpec::new("view.zoom_in", "view", "Zoom in", "Multiplies the zoom by 1.25.", &Typed::new(zoom_in), PALETTE)
        .shortcuts(ZOOM_IN_KEYS),
    CommandSpec::new("view.zoom_out", "view", "Zoom out", "Multiplies the zoom by 0.8.", &Typed::new(zoom_out), PALETTE)
        .shortcuts(ZOOM_OUT_KEYS),
    CommandSpec::new(
        "view.zoom_reset",
        "view",
        "Reset zoom",
        "Sets the zoom to 100% (1.68), optionally moving the content centre to the viewport centre.",
        &Typed::new(zoom_reset),
        PALETTE,
    )
    .shortcuts(ZOOM_RESET_KEYS),
    CommandSpec::new(
        "view.zoom_to_fit",
        "view",
        "Zoom to fit",
        "Fits the selection, all content, some elements or a rectangle into the viewport.",
        &Typed::new(zoom_to_fit),
        Facts { needs_selection: crate::spec::Selection::Soft, ..PALETTE },
    )
    .refusals(&["nothing_to_do"])
    .shortcuts(FIT_KEYS)
    .example(r#"{"scope":"all","padding":50}"#),
    CommandSpec::new(
        "view.center",
        "view",
        "Centre view",
        "Moves the camera to the origin, the content, the selection or a point, keeping the zoom.",
        &Typed::new(center),
        PALETTE,
    )
    .refusals(&["nothing_to_do"])
    .example(r#"{"on":"content"}"#),
    CommandSpec::new(
        "view.scroll_page",
        "view",
        "Scroll by page",
        "Scrolls by a fraction of the viewport in one direction.",
        &Typed::new(scroll_page),
        Facts::CAMERA,
    )
    .shortcuts(SCROLL_KEYS)
    .example(r#"{"direction":"down","fraction":0.8}"#),
    CommandSpec::new(
        "view.set_viewport",
        "view",
        "Set viewport size",
        "Tells the engine the canvas size in logical pixels. The shell calls it on resize.",
        &Typed::new(set_viewport),
        Facts { agent: false, ..QUIET },
    )
    .example(r#"{"width":1280,"height":720,"scale_factor":2}"#),
    CommandSpec::new(
        "view.set_view_only",
        "view",
        "View-only mode",
        "Turns view-only mode on or off (omitted toggles). While on, commands that change the document refuse.",
        &Typed::new(set_view_only),
        Facts { redraw: crate::spec::Redraw::Overlay, palette: true, ..Facts::CAMERA },
    )
    .shortcuts(VIEW_ONLY_KEYS)
    .example(r#"{"on":true}"#),
    CommandSpec::new(
        "view.get",
        "view",
        "Get view",
        "Returns the camera, the viewport, the visible rectangle, view-only mode and the tool.",
        &Typed::new(get),
        Facts { palette: true, ..Facts::READ_ONLY },
    ),
    CommandSpec::new(
        "view.screenshot",
        "view",
        "Screenshot",
        "Renders a PNG of a region so that an agent can check what it drew. Never writes files.",
        &Typed::new(screenshot),
        Facts { ports: crate::spec::PortSet::RENDERER, ..Facts::READ_ONLY },
    )
    .refusals(&["nothing_to_do"])
    .example(r#"{"region":"all","max_side_px":1024}"#),
];

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
