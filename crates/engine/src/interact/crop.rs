//! The crop editor: what the pointer and the keys do while an image is being cropped
//! (`docs/exceptions.md` E17, `docs/interaction.md`).
//!
//! `image.crop_begin` opens the editor (a mode recorded in the session's observed state). From then on this
//! machine owns the pointer:
//!
//! * The **window** is the part of the picture that is shown; it starts as the element's box. Eight handles
//!   resize it. Shift keeps its aspect ratio. An edge that comes close to an edge of the picture snaps to it.
//! * A drag that starts anywhere else on the picture **pans** the picture under the window, which does not move.
//! * Nothing in the document changes while the pointer moves (class B). The editor keeps the window and the
//!   shift of the picture; the overlay shows the whole picture faded with the draft drawn over it.
//! * `Enter`, `Esc` and a click outside the window end the editor and commit what was done with one
//!   `image.crop` (Excalidraw keeps the crop on `Esc` too). Nothing was done: `image.crop_end`.
//!
//! All the geometry is in the element's local frame: origin at the top-left of the unrotated box, no mirroring.
//! `drafftink_edit::crop::Picture` knows how the picture lies in it, so a rotated or mirrored image is cropped in
//! the way the user sees it. The pointer is converted into that frame, which is the only place rotation matters.

use drafftink_canvas::{Handle, SelectionFrame, Target};
use drafftink_edit::crop::{MIN_CROP_PX, Picture};
use drafftink_geom::OrientedBox;
use drafftink_geom::kurbo::{Point, Rect, Size, Vec2};
use drafftink_geom::select::HandleSet;
use drafftink_scene::{Element, ElementId, ElementKind, Kind};
use serde_json::json;

use super::overlay::CropView;
use crate::cmd::image::CropArgs;
use crate::cmd::view::RectArg;
use crate::interact::ctx::Ctx;
use crate::interact::events::{Ev, Key};
use crate::session::{Ref, Session};
use crate::state::Tool;

/// An edge of the window comes to the edge of the picture when it is this close (screen pixels).
const SNAP_PX: f64 = 6.0;
/// The window is not made smaller than this (world units), as in Excalidraw (`MINIMAL_CROP_SIZE`). A window that
/// is smaller than this to begin with keeps its size as the limit.
const MIN_WINDOW: f64 = 10.0;

/// The handles of the crop window: the corners and the middle of the edges.
pub(crate) const HANDLES: HandleSet = HandleSet { corners: true, edges: true, rotate: false };

/// Which edges of the window a handle moves: `-1` the left or top one, `1` the right or bottom one, `0` neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sides {
    x: i8,
    y: i8,
}

impl Sides {
    fn of(handle: &Handle) -> Option<Self> {
        let (x, y) = match handle {
            Handle::TopLeft => (-1, -1),
            Handle::Top => (0, -1),
            Handle::TopRight => (1, -1),
            Handle::Right => (1, 0),
            Handle::BottomRight => (1, 1),
            Handle::Bottom => (0, 1),
            Handle::BottomLeft => (-1, 1),
            Handle::Left => (-1, 0),
            Handle::Rotate | Handle::Point { .. } | Handle::Midpoint { .. } => return None,
        };
        Some(Self { x, y })
    }
}

/// The editor's draft of one image.
#[derive(Debug, Clone)]
struct Draft {
    /// The image as it was when the editor opened.
    element: Element,
    /// Where its whole picture lay then, in the local frame.
    picture: Picture,
    /// The window now.
    window: Rect,
    /// How far the picture was slid under the window.
    pan: Vec2,
}

impl Draft {
    fn open(e: &Element) -> Option<Self> {
        let picture = Picture::of(e).ok()?;
        Some(Self { element: e.clone(), picture, window: Self::box_of(e), pan: Vec2::ZERO })
    }

    fn box_of(e: &Element) -> Rect {
        Rect::from_origin_size(Point::ZERO, Size::new(e.transform.w, e.transform.h))
    }

    /// The picture as the draft has it, in the local frame.
    fn shifted(&self) -> Picture {
        self.picture.shifted(self.pan.x, self.pan.y)
    }

    fn is_changed(&self) -> bool {
        self.pan != Vec2::ZERO || self.window != Self::box_of(&self.element)
    }

    /// The pointer in the local frame.
    fn local(&self, world: Point) -> Point {
        self.element.transform.world_to_local(world)
    }

    /// A local rectangle as a world box that turns with the element.
    fn world_box(&self, local: Rect) -> OrientedBox {
        let t = &self.element.transform;
        OrientedBox::new(t.local_to_world(local.center()), local.size(), t.angle)
    }

    /// The unrotated world rectangle of the window: the element's box if the draft were committed.
    fn world_bounds(&self) -> Rect {
        Rect::from_center_size(self.element.transform.local_to_world(self.window.center()), self.window.size())
    }

    /// The window as the handles of the selection frame see it.
    fn frame(&self) -> SelectionFrame {
        SelectionFrame::Box { obox: self.world_box(self.window), set: HANDLES, multi: false }
    }

    /// The image as the draft would leave it.
    fn shown(&self) -> Element {
        let mut e = self.element.clone();
        let b = self.world_bounds();
        (e.transform.x, e.transform.y, e.transform.w, e.transform.h) = (b.x0, b.y0, b.width(), b.height());
        if let ElementKind::Image(d) = &mut e.kind {
            d.crop = self.shifted().crop_of(self.window);
        }
        e
    }

    /// The whole picture as the draft has it, to draw faded under the window.
    fn whole(&self) -> Element {
        let mut e = self.element.clone();
        let whole = self.shifted().whole();
        let t = &self.element.transform;
        let centre = t.local_to_world(whole.center());
        (e.transform.x, e.transform.y) = (centre.x - whole.width() / 2.0, centre.y - whole.height() / 2.0);
        (e.transform.w, e.transform.h) = (whole.width(), whole.height());
        if let ElementKind::Image(d) = &mut e.kind {
            d.crop = None;
        }
        e
    }

    fn view(&self) -> CropView {
        CropView {
            element: self.element.id.clone(),
            shown: self.shown(),
            whole: self.whole(),
            window: self.world_box(self.window),
            picture: self.world_box(self.shifted().whole()),
        }
    }

    /// The arguments of the one `image.crop` that commits the draft.
    fn commit_args(&self) -> CropArgs {
        let rect = |r: Rect| RectArg { x: r.x0, y: r.y0, w: r.width(), h: r.height() };
        let picture = self.shifted();
        CropArgs {
            id: Some(Ref(self.element.id.as_str().to_owned())),
            crop: picture.crop_of(self.window).map(|c| rect(c.rect())),
            bounds: Some(rect(self.world_bounds())),
            ..CropArgs::default()
        }
    }

    // ---- editing ----

    /// Moves the edges a handle holds by `delta`, how far the pointer went since the press (local frame), so the
    /// edge keeps its distance from the finger that took it. `snap` is the reach of the snap in local units.
    fn resize(&mut self, from: Rect, sides: Sides, delta: Vec2, keep_aspect: bool, snap: f64) {
        let pic = self.shifted().whole();
        let (kx, ky) = self.picture.scale();
        let min_w = MIN_WINDOW.min(from.width()).max(kx * MIN_CROP_PX);
        let min_h = MIN_WINDOW.min(from.height()).max(ky * MIN_CROP_PX);
        let near = |v: f64, edge: f64| if (v - edge).abs() <= snap { edge } else { v };
        let mut w = from;
        if sides.x < 0 {
            w.x0 = limit(near(from.x0 + delta.x, pic.x0), pic.x0, from.x1 - min_w);
        } else if sides.x > 0 {
            w.x1 = limit(near(from.x1 + delta.x, pic.x1), from.x0 + min_w, pic.x1);
        }
        if sides.y < 0 {
            w.y0 = limit(near(from.y0 + delta.y, pic.y0), pic.y0, from.y1 - min_h);
        } else if sides.y > 0 {
            w.y1 = limit(near(from.y1 + delta.y, pic.y1), from.y0 + min_h, pic.y1);
        }
        self.window = if keep_aspect { keep_ratio(from, w, sides, pic, (min_w, min_h)) } else { w };
    }

    /// Slides the picture to `start + delta` (local frame), so that it still covers the window. An edge of the
    /// picture that comes close to the window's snaps to it.
    fn pan_to(&mut self, start: Vec2, delta: Vec2, snap: f64) {
        let p0 = self.picture.whole();
        let w = self.window;
        // The picture covers the window while its offset stays in this range.
        let (lo_x, hi_x) = (w.x1 - p0.x1, w.x0 - p0.x0);
        let (lo_y, hi_y) = (w.y1 - p0.y1, w.y0 - p0.y0);
        let want = start + delta;
        let pick = |v: f64, lo: f64, hi: f64| {
            limit(
                if (v - lo).abs() <= snap {
                    lo
                } else if (v - hi).abs() <= snap {
                    hi
                } else {
                    v
                },
                lo,
                hi,
            )
        };
        self.pan = Vec2::new(pick(want.x, lo_x, hi_x), pick(want.y, lo_y, hi_y));
    }
}

/// `v` limited to `lo..=hi`. When the range is empty the upper end wins.
fn limit(v: f64, lo: f64, hi: f64) -> f64 {
    if lo > hi { hi } else { v.clamp(lo, hi) }
}

/// The window `moved` with its aspect ratio set back to that of `from`, anchored where the handle's opposite
/// side is, and kept inside the picture `pic`. A side handle changes the other axis about the window's middle.
fn keep_ratio(from: Rect, moved: Rect, sides: Sides, pic: Rect, min: (f64, f64)) -> Rect {
    let ratio = from.width() / from.height();
    if !ratio.is_finite() || ratio <= 0.0 {
        return moved;
    }
    let anchor_x = match sides.x {
        s if s < 0 => from.x1,
        s if s > 0 => from.x0,
        _ => from.center().x,
    };
    let anchor_y = match sides.y {
        s if s < 0 => from.y1,
        s if s > 0 => from.y0,
        _ => from.center().y,
    };
    // How far the picture reaches from the anchor on each axis, and the width each allows.
    let reach_x = match sides.x {
        s if s < 0 => anchor_x - pic.x0,
        s if s > 0 => pic.x1 - anchor_x,
        _ => 2.0 * (anchor_x - pic.x0).min(pic.x1 - anchor_x),
    };
    let reach_y = match sides.y {
        s if s < 0 => anchor_y - pic.y0,
        s if s > 0 => pic.y1 - anchor_y,
        _ => 2.0 * (anchor_y - pic.y0).min(pic.y1 - anchor_y),
    };
    let wanted_w = match (sides.x, sides.y) {
        (0, _) => moved.height() * ratio,
        (_, 0) => moved.width(),
        _ => moved.width().max(moved.height() * ratio),
    };
    let w = limit(wanted_w, min.0.max(min.1 * ratio), reach_x.min(reach_y * ratio));
    let h = w / ratio;
    let span = |anchor: f64, side: i8, len: f64| match side {
        s if s < 0 => (anchor - len, anchor),
        s if s > 0 => (anchor, anchor + len),
        _ => (anchor - len / 2.0, anchor + len / 2.0),
    };
    let (x0, x1) = span(anchor_x, sides.x, w);
    let (y0, y1) = span(anchor_y, sides.y, h);
    Rect::new(x0, y0, x1, y1)
}

/// What the pointer is doing in the editor.
#[derive(Debug, Default)]
enum Drag {
    #[default]
    Idle,
    /// A handle holds some edges of the window. `from` is the window at the press.
    Resize { sides: Sides, from: Rect },
    /// The picture is being slid. `start` is its shift at the press.
    Pan { start: Vec2 },
    /// The press landed on nothing the editor can use.
    Dead,
}

/// The crop editor of the interaction layer.
#[derive(Debug, Default)]
pub(crate) struct CropTool {
    draft: Option<Draft>,
    drag: Drag,
    /// The window and the shift at the start of the running drag, for a cancel.
    restore: Option<(Rect, Vec2)>,
}

impl CropTool {
    /// Follows the session: the editor is open exactly when the session says so, on the image it names. A draft
    /// whose image changed under it (undo, a collaborator) is made again from the image as it is now.
    pub(crate) fn sync(&mut self, s: &Session) {
        let open = s.tab().observed.crop_editor.as_ref().and_then(|id| s.element(id));
        let fresh = match (&self.draft, open) {
            (Some(d), Some(e)) => d.element.id == e.id && d.element.version == e.version,
            (None, None) => true,
            _ => false,
        };
        if fresh {
            return;
        }
        self.drag = Drag::Idle;
        self.restore = None;
        self.draft = open.and_then(Draft::open);
    }

    /// True while the editor is open.
    pub(crate) fn is_open(&self) -> bool {
        self.draft.is_some()
    }

    /// True while a drag of the editor runs.
    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.drag, Drag::Idle)
    }

    /// The window as a selection frame, for the picker's handle hits.
    pub(crate) fn frame(&self) -> Option<SelectionFrame> {
        self.draft.as_ref().map(Draft::frame)
    }

    /// What the overlay shows. The editor may have been opened by a command since the last pointer event (the
    /// strip's button), so a draft that is not there yet is made from the image as it is.
    pub(crate) fn view(&self, s: &Session) -> Option<CropView> {
        let id = s.tab().observed.crop_editor.as_ref()?;
        match &self.draft {
            Some(d) if d.element.id == *id => Some(d.view()),
            _ => s.element(id).and_then(Draft::open).map(|d| d.view()),
        }
    }

    // ---- pointer ----

    /// A drag starts. A handle resizes the window; the picture elsewhere is slid; anything else is ignored.
    pub(crate) fn drag_start(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let Some(draft) = &self.draft else { return };
        self.restore = Some((draft.window, draft.pan));
        self.drag = match ev.target {
            Target::Handle(h) => Sides::of(h).map_or(Drag::Dead, |sides| Drag::Resize { sides, from: draft.window }),
            _ if draft.shifted().whole().contains(draft.local(ev.origin.world)) => Drag::Pan { start: draft.pan },
            _ => Drag::Dead,
        };
        cx.overlay_changed();
    }

    pub(crate) fn drag_update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let snap = cx.px(SNAP_PX);
        let keep_aspect = cx.mods.shift;
        let Some(draft) = &mut self.draft else { return };
        // How far the pointer went since the press, in the element's frame.
        let delta = draft.local(ev.at.world) - draft.local(ev.origin.world);
        match self.drag {
            Drag::Resize { sides, from } => draft.resize(from, sides, delta, keep_aspect, snap),
            Drag::Pan { start } => draft.pan_to(start, delta, snap),
            Drag::Idle | Drag::Dead => return,
        }
        cx.overlay_changed();
    }

    pub(crate) fn drag_end(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>, cancelled: bool) {
        if cancelled {
            self.cancel();
        } else {
            self.drag_update(cx, ev);
        }
        self.drag = Drag::Idle;
        self.restore = None;
        cx.overlay_changed();
    }

    /// A click. Inside the window nothing happens. Outside it the crop is committed and the editor closes; the
    /// result says the click should go on to the select tool (which selects what is under it).
    pub(crate) fn click(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) -> bool {
        let Some(draft) = &self.draft else { return false };
        if draft.window.contains(draft.local(ev.at.world)) {
            return false;
        }
        self.finish(cx);
        true
    }

    // ---- keys ----

    /// `Enter` and `Esc` end the editor with what was done. `Enter` on a selected image opens it.
    pub(crate) fn key(&mut self, cx: &mut Ctx<'_>, key: Key) -> bool {
        if self.is_open() {
            self.finish(cx);
            return true;
        }
        match key {
            Key::Enter => open_on_selection(cx),
            Key::Escape => false,
        }
    }

    /// Ends the editor: one `image.crop` for a draft that changed something, else `image.crop_end`.
    fn finish(&mut self, cx: &mut Ctx<'_>) {
        let Some(draft) = self.draft.take() else { return };
        self.drag = Drag::Idle;
        self.restore = None;
        let committed = draft.is_changed() && cx.exec_typed("image.crop", &draft.commit_args()).is_some();
        if !committed && cx.s.tab().observed.crop_editor.is_some() {
            // Nothing to commit, or the commit was refused (the refusal is shown): do not leave the editor open.
            cx.exec("image.crop_end", json!({}));
        }
        cx.overlay_changed();
    }

    /// Ends a running drag as if it was cancelled: the window and the picture go back to where the press found
    /// them. The draft stays; a tool change closes the editor through the session, and `sync` drops the draft.
    pub(crate) fn cancel(&mut self) {
        if let (Some(draft), Some((window, pan))) = (&mut self.draft, self.restore.take()) {
            (draft.window, draft.pan) = (window, pan);
        }
        self.drag = Drag::Idle;
    }
}

/// `Enter` with one image selected opens the editor on it. Returns whether the key was used.
fn open_on_selection(cx: &mut Ctx<'_>) -> bool {
    if !matches!(cx.tool(), Tool::Select | Tool::Lasso) {
        return false;
    }
    let one: Option<ElementId> = {
        let mut live = cx.s.selection().iter().filter(|id| cx.s.element(id).is_some());
        match (live.next(), live.next()) {
            (Some(id), None) => Some(id.clone()),
            _ => None,
        }
    };
    let Some(id) = one else { return false };
    if cx.s.element(&id).map(Element::tag) != Some(Kind::Image) {
        return false;
    }
    // The image is selected: the key is ours even when the editor refuses (the refusal is shown once).
    cx.exec("image.crop_begin", json!({ "id": id.as_str() }));
    true
}

/// Opens the editor on `id` (a double-click on an image).
pub(crate) fn open(cx: &mut Ctx<'_>, id: &ElementId) {
    cx.exec("image.crop_begin", json!({ "id": id.as_str() }));
}
