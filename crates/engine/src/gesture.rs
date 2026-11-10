//! The gesture protocol (docs/commands.md §1.10): `begin`, `update`, `end`, `cancel`.
//!
//! A gesture is one user action that streams many small updates and ends as one undo step. The engine
//! keeps the document position from before `begin`. Every frame restores that position and re-runs the
//! command live with absolute arguments, so dropped or reordered frames are harmless. `end` restores the
//! base once more and runs the command through the ordinary pipeline, so
//! `begin + updates + end(args)` equals `execute(command, args)` by construction.
//!
//! Two doors lead here. Shells call the typed methods (`gesture_begin_typed`, `gesture_update`,
//! `gesture_end_typed`, `gesture_cancel`) and build no JSON per pointer event. Scripts and the bridge call
//! the `gesture.*` commands, which wrap the same code. The ring gets one entry per gesture, written at
//! `end` or `cancel`, with the frame count.

use std::any::Any;
use std::fmt;

use serde_json::{Value, json};

use crate::executor::{Input, Mode, merge_common};
use crate::refusal::Refusal;
use crate::ring::{Origin, Outcome};
use crate::session::{Session, SoftState};
use crate::spec::CommandSpec;
use crate::state::TabId;
use crate::store::DocMark;
use drafftink_profile::now_ns;

/// A running gesture has to receive a frame within this many milliseconds, or the watchdog cancels it.
pub const WATCHDOG_MS: u64 = 30_000;

/// Names a running gesture, `g1`, `g2`, ...
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GestureId(pub u32);

impl fmt::Display for GestureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "g{}", self.0)
    }
}

impl GestureId {
    /// Parses `g12`.
    pub fn parse(s: &str) -> Option<Self> {
        s.strip_prefix('g').and_then(|n| n.parse().ok()).map(GestureId)
    }
}

/// The state of the one running gesture.
pub(crate) struct GestureState {
    id: GestureId,
    spec: &'static CommandSpec,
    tab: TabId,
    mark: DocMark,
    base: SoftState,
    last_args: Box<dyn Any>,
    frames: usize,
    last_activity_ms: u64,
    origin: Origin,
    started_ns: u64,
}

impl GestureState {
    pub(crate) fn describe(&self) -> Value {
        json!({ "gesture": self.id.to_string(), "command": self.spec.id, "frames": self.frames })
    }
}

fn no_gesture(command: &str, id: &str) -> Refusal {
    Refusal::custom(
        "no_gesture",
        command,
        format!("There is no running gesture `{}`. Start one with `gesture.begin`.", crate::refusal::echo(id)),
    )
    .suggesting("gesture.begin", None)
}

fn unknown_gesture_id(command: &str, s: &str) -> Refusal {
    no_gesture(command, s)
}

impl Session {
    // ---- typed fast path ----

    /// Begins a gesture with typed arguments. `args` must be the command's argument struct.
    pub fn gesture_begin_typed(
        &mut self,
        origin: Origin,
        command: &str,
        args: &dyn Any,
    ) -> Result<(GestureId, Value), Refusal> {
        let r = self.begin_core(origin, command, Input::Typed(args));
        self.record_typed_refusal(origin, "gesture.begin", &r);
        r
    }

    /// Sends one live frame with typed, absolute arguments. A refusal keeps the last good frame.
    pub fn gesture_update(&mut self, id: GestureId, args: &dyn Any) -> Result<Value, Refusal> {
        let origin = self.gesture.as_ref().map_or(Origin::Ui, |g| g.origin);
        let r = self.update_core(id, Input::Typed(args));
        self.record_typed_refusal(origin, "gesture.update", &r);
        r
    }

    /// Ends the gesture with typed arguments (default: the last frame's) and commits one undo step.
    pub fn gesture_end_typed(&mut self, id: GestureId, args: Option<&dyn Any>) -> Result<Value, Refusal> {
        let origin = self.gesture.as_ref().map_or(Origin::Ui, |g| g.origin);
        let r = self.end_core(Some(id), args.map(Input::Typed));
        if r.is_err() {
            self.record_typed_refusal(origin, "gesture.end", &r);
        }
        r
    }

    /// Cancels the gesture and restores the state from before `begin`. With none running it is a no-op.
    pub fn gesture_cancel(&mut self, id: Option<GestureId>) -> Result<Value, Refusal> {
        let origin = self.gesture.as_ref().map_or(Origin::Ui, |g| g.origin);
        let r = self.cancel_core(id, origin);
        self.record_typed_refusal(origin, "gesture.cancel", &r);
        r
    }

    /// Cancels a running gesture if there is one (view-only switch, tab switch, blur).
    pub fn cancel_gesture_if_running(&mut self) -> bool {
        self.gesture.is_some() && self.cancel_core(None, Origin::Ui).is_ok()
    }

    /// Housekeeping the shell calls on a timer: cancels a gesture that got no frame for
    /// [`WATCHDOG_MS`]. Returns true when it cancelled one.
    pub fn tick(&mut self) -> bool {
        let now = self.ports.clock.mono_ms();
        let stale = self.gesture.as_ref().is_some_and(|g| now.saturating_sub(g.last_activity_ms) >= WATCHDOG_MS);
        stale && self.cancel_core(None, Origin::Ui).is_ok()
    }

    /// The running gesture's id.
    pub fn running_gesture(&self) -> Option<GestureId> {
        self.gesture.as_ref().map(|g| g.id)
    }

    fn record_typed_refusal(&mut self, origin: Origin, id: &str, r: &Result<impl Sized, Refusal>) {
        // Only for calls that did not come through `execute_spec` (depth 0 means no command is running).
        if self.run.depth == 0
            && let Err(e) = r
        {
            self.record_refusal(origin, id, None, e, 0);
        }
    }

    // ---- the JSON door (used by the `gesture.*` commands) ----

    pub(crate) fn gesture_begin_json(&mut self, command: &str, args: &Value) -> Result<Value, Refusal> {
        let origin = self.run.origin;
        let (id, frame) = self.begin_core(origin, command, Input::Json(args))?;
        Ok(with_gesture_key(frame, id))
    }

    pub(crate) fn gesture_update_json(&mut self, gesture: &str, args: &Value) -> Result<Value, Refusal> {
        let id = GestureId::parse(gesture).ok_or_else(|| unknown_gesture_id("gesture.update", gesture))?;
        let frame = self.update_core(id, Input::Json(args))?;
        Ok(with_gesture_key(frame, id))
    }

    pub(crate) fn gesture_end_json(&mut self, gesture: Option<&str>, args: Option<&Value>) -> Result<Value, Refusal> {
        let id = match gesture {
            Some(g) => Some(GestureId::parse(g).ok_or_else(|| unknown_gesture_id("gesture.end", g))?),
            None => None,
        };
        self.end_core(id, args.map(Input::Json))
    }

    pub(crate) fn gesture_cancel_json(&mut self, gesture: Option<&str>) -> Result<Value, Refusal> {
        // A cancel that names no known gesture is a stray one (window blur): nothing to cancel.
        let id = gesture.and_then(GestureId::parse);
        if gesture.is_some() && id.is_none() {
            return Ok(json!({ "changed": false, "cancelled": false }));
        }
        let origin = self.run.origin;
        self.cancel_core(id, origin)
    }

    // ---- the machinery ----

    fn gesture_commands(&self) -> String {
        let ids: Vec<&str> = self.registry().all().iter().filter(|s| s.is_gesture()).map(|s| s.id).collect();
        if ids.is_empty() { "(no command is registered as a gesture yet)".to_owned() } else { ids.join(", ") }
    }

    fn begin_core(&mut self, origin: Origin, command: &str, input: Input<'_>) -> Result<(GestureId, Value), Refusal> {
        if self.gesture.is_some() {
            return Err(Refusal::gesture_active("gesture.begin"));
        }
        let Some(spec) = self.registry().find(command) else {
            return Err(self.registry().unknown(command).raised_by("gesture.begin"));
        };
        if !spec.is_gesture() {
            return Err(Refusal::custom(
                "not_gesturable",
                "gesture.begin",
                format!(
                    "`{}` cannot run as a gesture. Gestures work with: {}.",
                    crate::refusal::echo(command),
                    self.gesture_commands()
                ),
            ));
        }
        self.check_gates(spec, origin, Mode::Top, true)?;
        self.check_gesture_available(spec)?;
        let args: Box<dyn Any> = match input {
            Input::Json(v) => spec.handler.parse(spec.id, v)?,
            Input::Typed(a) => spec.handler.clone_args(a).ok_or_else(|| {
                Refusal::internal("gesture.begin", format!("the typed arguments do not match `{}`", spec.id))
            })?,
        };
        let tab = self.tab().id;
        let mark = self.tab().doc.mark();
        let base = self.soft_state();
        self.reset_run_buffers();
        let frame = self.run_live_frame(spec, args.as_ref());
        match frame {
            Err(r) => {
                let r = self.rollback(tab, mark, &base, r);
                self.run.warnings.clear();
                Err(r.for_command(spec.id))
            }
            Ok(out) => {
                let id = GestureId(self.next_gesture);
                self.next_gesture += 1;
                let now = self.ports.clock.mono_ms();
                let out = self.frame_output(tab, mark, out);
                self.gesture = Some(GestureState {
                    id,
                    spec,
                    tab,
                    mark,
                    base,
                    last_args: args,
                    frames: 1,
                    last_activity_ms: now,
                    origin,
                    started_ns: now_ns(),
                });
                self.mark_dirty(crate::spec::Redraw::Scene);
                // The ring gets one entry for the whole gesture, written at `end` or `cancel`.
                self.run.ring_handled = true;
                Ok((id, out))
            }
        }
    }

    fn check_gesture_available(&self, spec: &'static CommandSpec) -> Result<(), Refusal> {
        crate::executor::guard(spec.id, || (spec.available)(self)).map_err(|r| r.for_command(spec.id))
    }

    fn reset_run_buffers(&mut self) {
        self.run.warnings.clear();
        self.run.writes.clear();
        self.run.history_reset = false;
    }

    fn run_live_frame(&mut self, spec: &'static CommandSpec, args: &dyn Any) -> Result<Value, Refusal> {
        self.run.live = true;
        let r = self.invoke(spec, args);
        self.run.live = false;
        r
    }

    fn frame_output(&mut self, tab: TabId, mark: DocMark, out: Value) -> Value {
        let summary = self.tab_by_id(tab).map(|t| t.doc.summary_since(mark)).unwrap_or_default();
        let warnings = std::mem::take(&mut self.run.warnings);
        self.run.writes.clear();
        merge_common(out, summary.changed, None, &summary, warnings).0
    }

    fn update_core(&mut self, id: GestureId, input: Input<'_>) -> Result<Value, Refusal> {
        let Some(g) = self.gesture.as_ref() else {
            return Err(no_gesture("gesture.update", &id.to_string()));
        };
        if g.id != id {
            return Err(no_gesture("gesture.update", &id.to_string()));
        }
        let (spec, tab, mark) = (g.spec, g.tab, g.mark);
        let args: Box<dyn Any> = match input {
            Input::Json(v) => spec.handler.parse(spec.id, v)?,
            Input::Typed(a) => spec.handler.clone_args(a).ok_or_else(|| {
                Refusal::internal("gesture.update", format!("the typed arguments do not match `{}`", spec.id))
            })?,
        };
        let base = match self.gesture.as_ref() {
            Some(g) => g.base.clone(),
            None => return Err(no_gesture("gesture.update", &id.to_string())),
        };
        let pre = self.soft_state();
        let forward = match self.tab_by_id_mut(tab) {
            Some(t) => t
                .doc
                .rollback_to(mark)
                .map_err(|e| Refusal::internal(spec.id, format!("restoring the base failed: {e}")))?,
            None => {
                self.gesture = None;
                return Err(no_gesture("gesture.update", &id.to_string()));
            }
        };
        self.restore_soft(&base);
        self.reset_run_buffers();
        match self.run_live_frame(spec, args.as_ref()) {
            Ok(out) => {
                let out = self.frame_output(tab, mark, out);
                let now = self.ports.clock.mono_ms();
                if let Some(g) = self.gesture.as_mut() {
                    g.frames += 1;
                    g.last_args = args;
                    g.last_activity_ms = now;
                }
                self.mark_dirty(crate::spec::Redraw::Scene);
                Ok(out)
            }
            Err(r) => {
                // Keep the last good frame: undo the failed one, put the good one back.
                if let Some(t) = self.tab_by_id_mut(tab) {
                    let _ = t.doc.rollback_to(mark);
                    let _ = t.doc.reapply(&forward);
                }
                self.restore_soft(&pre);
                self.run.warnings.clear();
                self.run.writes.clear();
                Err(r.for_command(spec.id))
            }
        }
    }

    fn end_core(&mut self, id: Option<GestureId>, input: Option<Input<'_>>) -> Result<Value, Refusal> {
        let Some(g) = self.gesture.as_ref() else {
            return Err(no_gesture("gesture.end", &id.map_or_else(|| "(none)".to_owned(), |i| i.to_string())));
        };
        if id.is_some_and(|i| i != g.id) {
            return Err(no_gesture("gesture.end", &id.map_or_else(String::new, |i| i.to_string())));
        }
        let Some(g) = self.gesture.take() else {
            return Err(no_gesture("gesture.end", "(none)"));
        };
        // Back to the base, as if the gesture had never run.
        if let Some(t) = self.tab_by_id_mut(g.tab)
            && let Err(e) = t.doc.rollback_to(g.mark)
        {
            self.restore_soft(&g.base);
            return Err(Refusal::internal(
                "gesture.end",
                format!("restoring the base failed: {e}. Reload the document from the last save"),
            ));
        }
        self.restore_soft(&g.base);
        let spec = g.spec;
        let t0 = g.started_ns;
        self.run.gesture_end = true;
        let res = match input {
            Some(i) => self.run_pipeline(spec, i, g.origin, Mode::Top),
            None => self.run_pipeline(spec, Input::Typed(g.last_args.as_ref()), g.origin, Mode::Top),
        };
        self.run.gesture_end = false;
        let dur = now_ns().saturating_sub(t0) / 1000;
        let args_json = match input {
            Some(Input::Json(v)) => Some(v.clone()),
            Some(Input::Typed(a)) => spec.handler.to_json(a),
            None => spec.handler.to_json(g.last_args.as_ref()),
        };
        let outcome = match &res {
            Ok(d) => Outcome::ok(d.changed, d.undo_label.clone()),
            Err(r) => Outcome::refused(r),
        };
        self.record(g.origin, spec, args_json.as_ref(), outcome, dur, None, Some(g.frames));
        self.run.ring_handled = true;
        match res {
            Ok(d) => Ok(d.out),
            Err(r) => Err(r),
        }
    }

    fn cancel_core(&mut self, id: Option<GestureId>, origin: Origin) -> Result<Value, Refusal> {
        let Some(g) = self.gesture.as_ref() else {
            return Ok(json!({ "changed": false, "cancelled": false }));
        };
        if id.is_some_and(|i| i != g.id) {
            // A stray cancel for a gesture that already ended (window blur, a late pointer cancel).
            return Ok(json!({ "changed": false, "cancelled": false }));
        }
        let Some(g) = self.gesture.take() else {
            return Ok(json!({ "changed": false, "cancelled": false }));
        };
        let rolled = match self.tab_by_id_mut(g.tab) {
            Some(t) => t.doc.rollback_to(g.mark).map(|_| ()),
            None => Ok(()),
        };
        self.restore_soft(&g.base);
        if let Err(e) = rolled {
            return Err(Refusal::internal(
                "gesture.cancel",
                format!("restoring the base failed: {e}. Reload the document from the last save"),
            ));
        }
        self.run.warnings.clear();
        self.run.writes.clear();
        self.mark_dirty(crate::spec::Redraw::Scene);
        let dur = now_ns().saturating_sub(g.started_ns) / 1000;
        let args = json!({ "command": g.spec.id });
        if let Some(cancel) = self.registry().find("gesture.cancel") {
            self.record(origin, cancel, Some(&args), Outcome::ok(true, None), dur, None, Some(g.frames));
        }
        self.run.ring_handled = true;
        Ok(json!({ "changed": true, "cancelled": true, "command": g.spec.id, "frames": g.frames }))
    }
}

fn with_gesture_key(frame: Value, id: GestureId) -> Value {
    match frame {
        Value::Object(mut m) => {
            m.insert("gesture".to_owned(), Value::String(id.to_string()));
            Value::Object(m)
        }
        other => json!({ "gesture": id.to_string(), "value": other }),
    }
}
