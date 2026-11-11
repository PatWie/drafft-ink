//! Scrubbing controls (docs/exceptions.md E13): the one place that turns a drag on a slider into the
//! `gesture.*` commands.
//!
//! A slider calls [`slider`]; nothing else in the UI emits a [`Scrub`]. The helper and the frame
//! bookkeeping ([`ScrubTrack::settle`], called by `draw`) keep these rules, so that no control has to:
//!
//! * One drag is one gesture: `begin` on the first change with the pointer down, `update` for each further
//!   change, `end` when the pointer is no longer down on the slider.
//! * The slider stays enabled while its own gesture runs. Every other command is refused during a gesture, and
//!   a slider that greys itself for that reason loses the drag before its release can be seen.
//! * A gesture is never left open. If the slider is not drawn in a frame (the ribbon regrouped, the selection
//!   went away) or the window lost the focus, the frame ends the gesture there.
//! * Esc cancels the gesture and restores the old value; the slider ignores the rest of that drag.
//! * A change without the pointer (an arrow key, assistive technology) is a plain command, not a gesture.

use std::ops::RangeInclusive;

use egui::{Key, Response, Ui, WidgetInfo};
use serde_json::Value;

use crate::action::{Scrub, ScrubPhase, UiAction};
use crate::widgets::Cx;

/// A control that scrubs. The command of the gesture follows from the control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrubControl {
    /// The opacity slider of the ribbon.
    Opacity,
}

impl ScrubControl {
    /// The command the gesture runs and ends with.
    pub const fn command(self) -> &'static str {
        match self {
            Self::Opacity => "style.set",
        }
    }
}

impl Scrub {
    /// A step of the gesture of a control; the command comes from the control.
    pub fn new(phase: ScrubPhase, control: ScrubControl, args: Value) -> Self {
        Self { phase, control, command: control.command(), args }
    }
}

/// What a slider is, for [`slider`].
pub(crate) struct SliderSpec {
    /// Which control it is.
    pub control: ScrubControl,
    /// The accessible name and the tooltip.
    pub label: &'static str,
    /// The values it takes.
    pub range: RangeInclusive<f32>,
}

/// The scrubs of one frame as the widgets see them: a copy of the state at the start, and what the widgets
/// report back.
#[derive(Debug, Default)]
pub(crate) struct ScrubFrame {
    running: Option<ScrubControl>,
    suppressed: Option<ScrubControl>,
    seen: Vec<ScrubControl>,
    released: Option<ScrubControl>,
}

/// What the UI remembers of the scrub in progress.
#[derive(Debug, Default)]
pub(crate) struct ScrubTrack {
    /// The gesture a control began, with the control (`None`: the gesture is not a scrub's).
    running: Option<(String, Option<ScrubControl>)>,
    /// The control whose `begin` is on its way to the engine.
    starting: Option<ScrubControl>,
    /// The control that ignores the pointer until it is released: the engine refused or cancelled its gesture.
    suppressed: Option<ScrubControl>,
}

impl ScrubTrack {
    /// The id of the running scrub gesture.
    pub fn gesture(&self) -> Option<&str> {
        self.running.as_ref().map(|(id, _)| id.as_str())
    }

    /// The control whose gesture runs.
    pub fn control(&self) -> Option<ScrubControl> {
        self.running.as_ref().and_then(|(_, c)| *c)
    }

    /// Called as an action is routed, before it runs.
    pub fn routed(&mut self, scrub: &Scrub) {
        match scrub.phase {
            ScrubPhase::Begin => self.starting = Some(scrub.control),
            // The pointer is still down: the slider must not begin again with the next move.
            ScrubPhase::Cancel => self.suppressed = Some(scrub.control),
            ScrubPhase::Update | ScrubPhase::End => {}
        }
    }

    /// `gesture.begin` answered: `Some(id)` when it began.
    pub fn began(&mut self, gesture: Option<String>) {
        let control = self.starting.take();
        match gesture {
            Some(id) => self.running = Some((id, control)),
            None => self.suppressed = control,
        }
    }

    /// `gesture.update` was refused: the engine ended the gesture. Not worth beginning it again while the
    /// pointer is down.
    pub fn lost(&mut self) {
        self.suppressed = self.control().or(self.suppressed);
        self.running = None;
    }

    /// The gesture ended or was cancelled.
    pub fn ended(&mut self) {
        self.running = None;
    }

    /// What the widgets of this frame need to know.
    pub fn frame(&self) -> ScrubFrame {
        ScrubFrame { running: self.control(), suppressed: self.suppressed, ..ScrubFrame::default() }
    }

    /// After all widgets were drawn: forgets a released suppression and ends a gesture that has no slider
    /// to finish it (the slider is gone, or the window lost the focus: `focus_lost`), as if the pointer was released.
    pub fn settle(&mut self, frame: ScrubFrame, focus_lost: bool, out: &mut Vec<UiAction>) {
        if frame.released.is_some() && frame.released == self.suppressed {
            self.suppressed = None;
        }
        let Some(control) = self.control() else {
            return;
        };
        if focus_lost || !frame.seen.contains(&control) {
            out.push(UiAction::Scrub(Scrub::new(ScrubPhase::End, control, Value::Null)));
        }
    }
}

impl ScrubFrame {
    fn mark_seen(&mut self, control: ScrubControl) {
        if !self.seen.contains(&control) {
            self.seen.push(control);
        }
    }
}

/// The reason the command of a control cannot run now. A running gesture of the control itself is no
/// reason: it is the control's own drag.
fn refusal(cx: &Cx<'_>, control: ScrubControl) -> Option<String> {
    let id = control.command();
    let own = cx.scrub.running == Some(control);
    match cx.model.command(id)?.available.as_ref() {
        Err(r) if own && r.code == "gesture_active" => None,
        _ => cx.refusal(id),
    }
}

/// A slider that scrubs `spec.control`. `value` is the current value and is changed by the drag;
/// `args` builds the arguments of the command for a value. Returns the response of the widget.
pub(crate) fn slider(
    ui: &mut Ui,
    cx: &mut Cx<'_>,
    spec: &SliderSpec,
    value: &mut f32,
    args: impl Fn(f32) -> Value,
) -> Response {
    let SliderSpec { control, label, range } = spec;
    let control = *control;
    let blocked = refusal(cx, control);
    let resp = ui
        .add_enabled(blocked.is_none(), egui::Slider::new(value, range.clone()).show_value(false))
        .on_disabled_hover_text(blocked.clone().unwrap_or_default());
    let shown = f64::from(*value);
    resp.widget_info(|| WidgetInfo::slider(blocked.is_none(), shown, *label));
    let resp = if blocked.is_none() { resp.on_hover_text(*label) } else { resp };
    if ui.is_sizing_pass() {
        return resp;
    }
    cx.scrub.mark_seen(control);

    let held = resp.is_pointer_button_down_on() || resp.dragged();
    if cx.scrub.suppressed == Some(control) {
        if !held {
            cx.scrub.released = Some(control);
        }
        return resp;
    }
    let own = cx.scrub.running == Some(control);
    if own && ui.input(|i| i.key_pressed(Key::Escape)) {
        // Esc is also a shortcut (`tool.cancel`), which cancels the gesture in the engine too; this tells the
        // state and restores the value. The pointer is still down, so the slider ignores the rest of the drag.
        cx.emit(UiAction::Scrub(Scrub::new(ScrubPhase::Cancel, control, Value::Null)));
        cx.scrub.suppressed = Some(control);
        return resp;
    }
    let args = args(*value);
    if resp.changed() && held {
        let phase = if own { ScrubPhase::Update } else { ScrubPhase::Begin };
        cx.emit(UiAction::Scrub(Scrub::new(phase, control, args)));
    } else if resp.changed() {
        cx.run(control.command(), args);
    } else if own && !held {
        cx.emit(UiAction::Scrub(Scrub::new(ScrubPhase::End, control, args)));
    }
    resp
}
