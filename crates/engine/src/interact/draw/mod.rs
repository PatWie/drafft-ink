//! The drawing tools: shapes, lines and arrows, freehand, eraser, laser, text and math.
//!
//! Every tool here is class B or E of `docs/exceptions.md`. The document is untouched while the pointer is
//! down. A preview (an `Element` built with the session's style defaults), the faded elements of the eraser,
//! the smart guides and the binding highlight live in [`OverlayParts`](crate::interact::OverlayParts), and one
//! command runs at the end:
//!
//! | Tool | Gesture | Command at the end |
//! |---|---|---|
//! | rectangle, diamond, ellipse, frame | drag | `element.create` with `bounds` |
//! | line, arrow | drag, or clicks ended by double-click, Enter or Esc | `element.create` with `points` |
//! | freehand, highlighter | drag, or a click (a dot) | `element.create` kind freehand |
//! | eraser | drag, or a click | `element.erase` with the swept `path` |
//! | laser | drag | `collab.laser` (throttled, never stored) |
//! | text | click | `text.edit_begin`, then the shell opens the editor |
//! | math | click | `element.create` kind math, or `text.edit_begin` on an existing formula |
//!
//! Every `element.create` carries `from_tool: true`, so the engine returns the tool to select unless the tool
//! is locked.

mod eraser;
mod laser;
mod linear;
mod preview;
mod shape;
mod snap;
mod stroke;
mod text;

#[cfg(test)]
#[path = "../draw_line_tests.rs"]
mod line_tests;
#[cfg(test)]
#[path = "../draw_stroke_tests.rs"]
mod stroke_tests;
#[cfg(test)]
#[path = "../draw_tests.rs"]
mod tests;

use drafftink_geom::kurbo::Point;

use crate::interact::ctx::Ctx;
use crate::interact::events::{Ev, Key};
use crate::state::Tool;

/// The pointer drag that is running, other than a line or arrow (those keep their own state in [`linear`]).
#[derive(Debug)]
enum Gesture {
    Box(shape::BoxDrag),
    Stroke(stroke::Stroke),
    Sweep(eraser::Sweep),
    Laser,
    /// A drag of the text or math tool on empty canvas: it places at the origin when it ends.
    Place,
}

/// State of the drawing tools between pointer events.
#[derive(Debug, Default)]
pub(crate) struct DrawTools {
    gesture: Option<Gesture>,
    linear: linear::Linear,
    laser: laser::Laser,
    /// True while this machine has written preview, faded, guide or highlight parts that it has to take back.
    shown: bool,
}

impl DrawTools {
    pub(crate) fn press(&mut self, _cx: &mut Ctx<'_>, _ev: &Ev<'_>) {}

    pub(crate) fn click(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        match cx.tool() {
            Tool::Line | Tool::Arrow => self.linear.click(cx, ev, &mut self.shown),
            Tool::Freehand | Tool::Highlighter => stroke::dot(cx, ev),
            Tool::Eraser => eraser::tap(cx, ev),
            Tool::Text | Tool::Math => text::click(cx, ev),
            Tool::Select | Tool::Hand | Tool::Rectangle | Tool::Diamond | Tool::Ellipse | Tool::Frame | Tool::Laser => {
            }
        }
    }

    pub(crate) fn double_click(&mut self, cx: &mut Ctx<'_>, _ev: &Ev<'_>) {
        if matches!(cx.tool(), Tool::Line | Tool::Arrow) {
            self.linear.finish(cx, &mut self.shown);
        }
    }

    pub(crate) fn drag_start(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let tool = cx.tool();
        match tool {
            Tool::Rectangle | Tool::Diamond | Tool::Ellipse | Tool::Frame => {
                if let Some(drag) = shape::BoxDrag::start(tool, cx, ev) {
                    self.gesture = Some(Gesture::Box(drag));
                }
            }
            Tool::Line | Tool::Arrow => self.linear.drag_start(cx, ev),
            Tool::Freehand | Tool::Highlighter => {
                self.gesture = Some(Gesture::Stroke(stroke::Stroke::start(tool)));
            }
            Tool::Eraser => self.gesture = Some(Gesture::Sweep(eraser::Sweep::start())),
            Tool::Laser => {
                self.laser.begin(cx, ev);
                self.gesture = Some(Gesture::Laser);
            }
            Tool::Text | Tool::Math => self.gesture = Some(Gesture::Place),
            Tool::Select | Tool::Hand => {}
        }
        self.drag_update(cx, ev);
    }

    pub(crate) fn drag_update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        match &mut self.gesture {
            Some(Gesture::Box(drag)) => {
                drag.update(cx, ev);
                self.shown = true;
            }
            Some(Gesture::Stroke(stroke)) => {
                stroke.update(cx, ev);
                self.shown = true;
            }
            Some(Gesture::Sweep(sweep)) => {
                sweep.update(cx, ev);
                self.shown = true;
            }
            Some(Gesture::Laser) => self.laser.update(cx, ev),
            Some(Gesture::Place) => {}
            None => {
                if matches!(cx.tool(), Tool::Line | Tool::Arrow) {
                    self.linear.drag_update(cx, ev, &mut self.shown);
                }
            }
        }
    }

    pub(crate) fn drag_end(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>, cancelled: bool) {
        match self.gesture.take() {
            Some(Gesture::Box(drag)) => {
                self.take_back(cx);
                if !cancelled {
                    drag.finish(cx, ev);
                }
            }
            Some(Gesture::Stroke(stroke)) => {
                self.take_back(cx);
                if !cancelled {
                    stroke.finish(cx, ev);
                }
            }
            Some(Gesture::Sweep(sweep)) => {
                self.take_back(cx);
                if !cancelled {
                    sweep.finish(cx, ev);
                }
            }
            Some(Gesture::Laser) => self.laser.end(cx, ev, cancelled),
            Some(Gesture::Place) => {
                if !cancelled {
                    text::place(cx, ev.origin.world);
                }
            }
            None => {
                if matches!(cx.tool(), Tool::Line | Tool::Arrow) {
                    self.linear.drag_end(cx, ev, cancelled, &mut self.shown);
                }
            }
        }
    }

    /// The pointer moved with no button down: rubber band of a multi-point line, binding candidate.
    pub(crate) fn pointer_moved(&mut self, cx: &mut Ctx<'_>, _screen: Point, world: Point) {
        if matches!(cx.tool(), Tool::Line | Tool::Arrow) {
            self.linear.hover(cx, world, &mut self.shown);
        }
    }

    /// Drops the preview and any draft without a trace. The laser trail stays: it fades by the clock.
    pub(crate) fn cancel(&mut self, cx: &mut Ctx<'_>) {
        self.gesture = None;
        self.linear.reset();
        self.laser.cancel();
        self.take_back(cx);
    }

    /// Returns true when the key was used.
    pub(crate) fn key(&mut self, cx: &mut Ctx<'_>, key: Key) -> bool {
        match key {
            Key::Enter | Key::Escape if matches!(cx.tool(), Tool::Line | Tool::Arrow) => {
                self.linear.end_draft(cx, &mut self.shown)
            }
            Key::Enter | Key::Escape => false,
        }
    }

    /// Time passes: ages the laser trail and flushes the points not yet sent.
    pub(crate) fn tick(&mut self, cx: &mut Ctx<'_>) {
        self.laser.tick(cx);
    }

    /// When `tick` next has something to do (the laser fade), if ever.
    pub(crate) fn deadline(&self) -> Option<f64> {
        self.laser.deadline()
    }

    /// True while a drawing is in progress (a drag, or a multi-point line draft).
    pub(crate) fn is_active(&self) -> bool {
        self.gesture.is_some() || self.linear.is_active()
    }

    /// Removes what this machine put into the overlay parts.
    fn take_back(&mut self, cx: &mut Ctx<'_>) {
        if std::mem::take(&mut self.shown) {
            preview::clear(cx);
        }
    }
}
