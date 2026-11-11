//! The eraser (E8): the swept path fades what it touches, and one `element.erase` deletes it at the end.

use std::collections::BTreeSet;

use drafftink_geom::kurbo::Point;
use drafftink_scene::{ElementId, SceneRead, SceneStore};
use serde_json::json;

use crate::interact::ctx::Ctx;
use crate::interact::events::Ev;

/// The eraser's radius in screen pixels.
const RADIUS_PX: f64 = 10.0;
/// A path never holds more points than this (the command accepts 10 000).
const MAX_PATH: usize = 8_000;
/// One pointer step is probed at most this many times, however far the pointer jumped.
const MAX_PROBES: usize = 400;

/// The path swept so far and the elements it touches.
#[derive(Debug)]
pub(super) struct Sweep {
    path: Vec<Point>,
    touched: BTreeSet<ElementId>,
}

impl Sweep {
    pub(super) fn start() -> Self {
        Self { path: Vec::new(), touched: BTreeSet::new() }
    }

    /// Extends the path to the pointer and fades the elements the new part touches.
    pub(super) fn update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        let before = self.touched.len();
        self.begin(cx, ev);
        self.extend(cx, ev.at.world);
        if self.touched.len() != before {
            cx.parts.faded = self.touched.iter().cloned().collect();
            cx.overlay_changed();
        }
    }

    /// The press position is the first point of the path.
    fn begin(&mut self, cx: &Ctx<'_>, ev: &Ev<'_>) {
        if self.path.is_empty() {
            self.probe(cx, ev.origin.world);
            self.path.push(ev.origin.world);
        }
    }

    fn extend(&mut self, cx: &Ctx<'_>, to: Point) {
        let Some(from) = self.path.last().copied() else { return };
        if from.distance(to) < cx.px(1.0) {
            return;
        }
        let step = cx.px(RADIUS_PX / 2.0).max(f64::EPSILON);
        let n = ((from.distance(to) / step).ceil() as usize).clamp(1, MAX_PROBES);
        for k in 1..=n {
            self.probe(cx, from.lerp(to, k as f64 / n as f64));
        }
        if self.path.len() >= MAX_PATH {
            if let Some(slot) = self.path.last_mut() {
                *slot = to;
            }
        } else {
            self.path.push(to);
        }
    }

    /// Adds what the eraser touches at `p`, with the elements that go with it.
    fn probe(&mut self, cx: &Ctx<'_>, p: Point) {
        for hit in cx.world.hit_all(cx.s, p, RADIUS_PX) {
            if let Some(unit) = unit_of(cx, &hit.id) {
                self.touched.extend(unit);
            }
        }
    }

    /// The pointer was released: one `element.erase` along the whole path.
    pub(super) fn finish(mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.begin(cx, ev);
        self.extend(cx, ev.at.world);
        erase(cx, &self.path);
    }
}

/// A click without a drag erases at that point.
pub(super) fn tap(cx: &mut Ctx<'_>, ev: &Ev<'_>) {
    erase(cx, &[ev.at.world]);
}

fn erase(cx: &mut Ctx<'_>, path: &[Point]) {
    let pts: Vec<[f64; 2]> = path.iter().map(|p| [p.x, p.y]).collect();
    cx.exec("element.erase", json!({ "path": pts, "radius": RADIUS_PX, "units": "screen" }));
}

/// What erasing the element `id` removes, by the rules of `element.erase`: a label counts as its container,
/// and the container takes its labels and (for a frame) its children with it. `None` when any of them is
/// locked, because the command leaves such a unit alone.
fn unit_of(cx: &Ctx<'_>, id: &ElementId) -> Option<Vec<ElementId>> {
    let store = cx.s.store();
    let hit = store.get(id)?;
    let target = hit.container_id().and_then(|c| store.get(c)).filter(|c| !c.deleted).unwrap_or(hit);
    let derived = store.derived();
    let mut unit = vec![target.id.clone()];
    unit.extend(derived.labels_of(&target.id).cloned());
    for child in derived.frame_children(&target.id) {
        unit.push(child.clone());
        unit.extend(derived.labels_of(child).cloned());
    }
    let any_locked = unit.iter().any(|i| store.get(i).is_none_or(|e| e.locked));
    (!any_locked).then_some(unit)
}
