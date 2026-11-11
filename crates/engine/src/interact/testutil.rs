//! Test rig: a headless session and an [`Interaction`], driven by scripted pointer events.
//!
//! The rig puts the camera at zoom 1 with the world origin at the top-left of an 800 by 600 canvas, so a
//! screen position equals a world position and a test can say "press at (100, 100)" and mean both. Events
//! go through the real `Recognizer` of `drafftink-canvas` (`Interaction::input`).
//!
//! Commands the text and collab families will register (`text.edit_begin`, `collab.laser`) are stubbed in the
//! rig's registry when the global one lacks them, so tests can observe what the interaction layer asked for. A
//! real registration wins over the stub.

use std::sync::Arc;

use drafftink_canvas::{Button, Buttons, InputEvent, Modifiers, PointerEvent, PointerId, PointerKind, PointerPhase};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Color, Element, ElementId, SceneRead, SceneTx, StoreError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Interaction, Key, Outcome};
use crate::cmd::view::Pt;
use crate::ports::fake::Fakes;
use crate::refusal::Refusal;
use crate::registry::Registry;
use crate::ring::{Origin, RingEntry};
use crate::services::Services;
use crate::session::{Ref, Session};
use crate::spec::{CommandSpec, Facts, Mutates, Redraw, RingMode, Typed};
use crate::state::{TextEdit, Tool};
use crate::testing;

// ---- stubs for families that are not registered yet -----------------------------------------------------------

/// What the stub `collab.laser` received.
#[derive(Debug, Default, Clone)]
pub(super) struct LaserLog(pub(super) Vec<Vec<[f64; 2]>>);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub(super) struct StubEditBegin {
    pub id: Option<Ref>,
    pub at: Option<Pt>,
    pub container: Option<Ref>,
    pub caret: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub(super) struct StubLaser {
    pub points: Vec<Pt>,
    pub fade_ms: Option<u64>,
}

fn stub_edit_begin(s: &mut Session, a: StubEditBegin) -> Result<Value, Refusal> {
    let element = a.id.map(|r| s.resolve_ref(&r)).transpose()?;
    if let Some(id) = &element {
        s.tab_mut().observed.selection = std::iter::once(id.clone()).collect();
    }
    s.text_edit = Some(TextEdit { draft: element.is_none(), element });
    Ok(json!({}))
}

fn stub_laser(s: &mut Session, a: StubLaser) -> Result<Value, Refusal> {
    s.ext.with::<LaserLog, _>(|l| l.0.push(a.points));
    Ok(json!({}))
}

const STUB_FACTS: Facts = Facts {
    reads_only: false,
    mutates: Mutates::OBSERVED,
    redraw: Redraw::Overlay,
    ring: RingMode::Always,
    ..Facts::READ_ONLY
};

static STUBS: [CommandSpec; 2] = [
    CommandSpec::new(
        "text.edit_begin",
        "text",
        "Start editing text",
        "Stub of the text family for interaction tests: opens a text edit session.",
        &Typed::new(stub_edit_begin),
        STUB_FACTS,
    ),
    CommandSpec::new(
        "collab.laser",
        "collab",
        "Laser trail",
        "Stub of the collab family for interaction tests: records the trail.",
        &Typed::new(stub_laser),
        STUB_FACTS,
    ),
];

fn registry() -> Arc<Registry> {
    let mut families: Vec<&'static [CommandSpec]> = crate::cmd::FAMILIES.to_vec();
    families.push(&STUBS);
    Arc::new(Registry::from_families(&families))
}

// ---- the rig --------------------------------------------------------------------------------------------------

/// A session, its fakes, an interaction layer and a clock for scripted events.
pub(super) struct Rig {
    pub(super) s: Session,
    pub(super) fakes: Fakes,
    pub(super) i: Interaction,
    /// Time of the next event; every event adds 16 ms.
    pub(super) now: f64,
    /// Modifiers applied to every event.
    pub(super) mods: Modifiers,
    /// The outcome of the last call.
    pub(super) last: Outcome,
    /// Every outcome since the rig was made.
    pub(super) history: Vec<Outcome>,
    baseline_seq: u64,
}

impl Rig {
    /// A session with the fake text engine, an 800 by 600 canvas and the camera at zoom 1, scroll (400, 300).
    pub(super) fn new() -> Rig {
        let fakes = Fakes::new();
        let mut s = Session::with_registry(fakes.ports(), registry());
        s.services = Services::fake();
        let mut rig = Rig {
            s,
            fakes,
            i: Interaction::new(),
            now: 1000.0,
            mods: Modifiers::NONE,
            last: Outcome::default(),
            history: Vec::new(),
            baseline_seq: 0,
        };
        rig.run("view.set_viewport", json!({ "width": 800, "height": 600 }));
        rig.run("view.set", json!({ "scroll": [400, 300], "zoom": 1 }));
        rig.baseline_seq = rig.s.ring().next_seq();
        rig
    }

    /// Runs a command as a script and returns its output; panics on a refusal.
    pub(super) fn run(&mut self, id: &str, args: Value) -> Value {
        match self.s.execute_from(Origin::Test, id, args) {
            Ok(v) => v,
            Err(r) => panic!("{id} refused: {} ({})", r.message, r.code),
        }
    }

    /// Runs a command as a script and returns the refusal it must give.
    pub(super) fn refuse(&mut self, id: &str, args: Value) -> Refusal {
        match self.s.execute_from(Origin::Test, id, args) {
            Ok(v) => panic!("{id} should have refused, got {v}"),
            Err(r) => r,
        }
    }

    /// Switches tool the way a toolbar does.
    pub(super) fn tool(&mut self, tool: Tool) {
        self.run("tool.set", json!({ "tool": tool.name() }));
    }

    /// Adds a rectangle with a white fill, so that a click inside hits it. Not an undo step.
    pub(super) fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) -> ElementId {
        let id = testing::add_rect(&mut self.s, x, y, w, h).expect("add rectangle");
        self.s
            .tab_mut()
            .doc
            .edit::<(), StoreError>(|tx| tx.update(&id, |e| e.style.fill_color = Some(Color::rgb(255, 255, 255))))
            .expect("fill rectangle");
        self.s.tab_mut().doc.commit_untracked().expect("commit");
        id
    }

    // ---- scripted pointer input ----

    fn event(&mut self, phase: PointerPhase, x: f64, y: f64, button: Button, buttons: Buttons) -> Outcome {
        self.now += 16.0;
        self.fakes.clock.advance_ms(16);
        let ev = PointerEvent::new(PointerId(0), PointerKind::Mouse, phase, Point::new(x, y), self.now)
            .with_button(button, buttons)
            .with_modifiers(self.mods);
        self.input(InputEvent::Pointer(ev))
    }

    /// Feeds any input event and remembers the outcome.
    pub(super) fn input(&mut self, ev: InputEvent) -> Outcome {
        let out = self.i.input(&mut self.s, &ev);
        self.last = out.clone();
        self.history.push(out.clone());
        out
    }

    /// Mouse down with the primary button.
    pub(super) fn down(&mut self, x: f64, y: f64) -> Outcome {
        self.event(PointerPhase::Down, x, y, Button::Primary, Buttons::PRIMARY)
    }

    /// Mouse move with the primary button held.
    pub(super) fn drag_to(&mut self, x: f64, y: f64) -> Outcome {
        self.event(PointerPhase::Move, x, y, Button::None, Buttons::PRIMARY)
    }

    /// Mouse move with no button.
    pub(super) fn hover(&mut self, x: f64, y: f64) -> Outcome {
        self.event(PointerPhase::Move, x, y, Button::None, Buttons::NONE)
    }

    /// Mouse up with the primary button.
    pub(super) fn up(&mut self, x: f64, y: f64) -> Outcome {
        self.event(PointerPhase::Up, x, y, Button::Primary, Buttons::NONE)
    }

    /// Press, move in five steps, release.
    pub(super) fn drag(&mut self, from: (f64, f64), to: (f64, f64)) {
        self.down(from.0, from.1);
        for k in 1..=5 {
            let t = f64::from(k) / 5.0;
            self.drag_to(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        }
        self.up(to.0, to.1);
    }

    /// Press and release in place.
    pub(super) fn click(&mut self, x: f64, y: f64) {
        self.down(x, y);
        self.up(x, y);
    }

    /// Two clicks 100 ms apart.
    pub(super) fn double_click(&mut self, x: f64, y: f64) {
        self.click(x, y);
        self.now += 84.0;
        self.click(x, y);
    }

    /// Enter or Escape.
    pub(super) fn key(&mut self, key: Key) -> Outcome {
        self.now += 16.0;
        let out = self.i.key(&mut self.s, key, self.now);
        self.last = out.clone();
        self.history.push(out.clone());
        out
    }

    /// Time passes.
    pub(super) fn tick(&mut self, ms: f64) -> Outcome {
        self.now += ms;
        let out = self.i.tick(&mut self.s, self.now);
        self.last = out.clone();
        self.history.push(out.clone());
        out
    }

    // ---- reading ----

    /// The mutating ring entries since the rig was made, oldest first.
    pub(super) fn ring(&self) -> Vec<RingEntry> {
        self.s.ring().recent(256, true, Some(self.baseline_seq.saturating_sub(1)))
    }

    /// The ids of those entries.
    pub(super) fn ring_ids(&self) -> Vec<String> {
        self.ring().into_iter().map(|e| e.id).collect()
    }

    /// Every ring entry (reads and quiet ones too) since the rig was made.
    pub(super) fn ring_all_ids(&self) -> Vec<String> {
        self.s.ring().recent(256, false, Some(self.baseline_seq.saturating_sub(1))).into_iter().map(|e| e.id).collect()
    }

    /// The selected ids, sorted.
    pub(super) fn selected(&self) -> Vec<String> {
        self.s.selection().iter().map(|i| i.as_str().to_owned()).collect()
    }

    /// A live element.
    pub(super) fn element(&self, id: &ElementId) -> Element {
        self.s.element(id).cloned().expect("element exists")
    }

    /// The box of an element as `(x, y, w, h)`.
    pub(super) fn bounds(&self, id: &ElementId) -> (f64, f64, f64, f64) {
        let t = &self.element(id).transform;
        (t.x, t.y, t.w, t.h)
    }

    /// Every live element, back to front.
    pub(super) fn elements(&self) -> Vec<Element> {
        self.s.store().iter().cloned().collect()
    }

    /// The live elements as JSON in z order, with the parts that legitimately differ between two runs of the
    /// same edit removed (ids, versions, z keys, sketch seeds), and ids inside bindings and group lists
    /// replaced by their position in z order. Two documents that are equal after this are "the same document
    /// (ids and timestamps aside)" in the sense of docs/exceptions.md.
    pub(super) fn normalized_doc(&self) -> Value {
        let elements = self.elements();
        let position = |id: &str| elements.iter().position(|e| e.id.as_str() == id);
        let mut out = Vec::new();
        for e in &elements {
            let mut v = serde_json::to_value(e).expect("element serialises");
            strip_volatile(&mut v, &position);
            out.push(v);
        }
        Value::Array(out)
    }

    /// The number of undo steps the active tab holds.
    pub(super) fn undo_depth(&self) -> usize {
        self.s.tab().doc.undo_depth()
    }

    /// True when the rig's registry runs the stub of `id` (the real family is not registered).
    pub(super) fn stubbed(&self, id: &str) -> bool {
        STUBS.iter().any(|spec| spec.id == id)
            && crate::cmd::FAMILIES.iter().all(|fam| fam.iter().all(|spec| spec.id != id))
    }
}

/// Removes ids, versions, z keys and seeds from an element's JSON, and renames id references to positions.
fn strip_volatile(v: &mut Value, position: &dyn Fn(&str) -> Option<usize>) {
    match v {
        Value::Object(map) => {
            for key in ["id", "version", "index", "seed", "group_ids"] {
                map.remove(key);
            }
            for (key, val) in map.iter_mut() {
                if matches!(key.as_str(), "target" | "frame_id" | "container")
                    && let Value::String(s) = val
                {
                    *val = position(s).map_or(Value::Null, |p| json!(format!("#{p}")));
                } else {
                    strip_volatile(val, position);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| strip_volatile(i, position)),
        _ => {}
    }
}
