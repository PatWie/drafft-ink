//! The built-in pointer behaviour: select, move, transform, rubber band.
//!
//! The canvas crate only says what happened (`Press`, `DragUpdate` on a handle, ...). A host without an editor
//! wants the obvious thing to happen, so this module acts on those intents with the editor's own rules from
//! `drafftink-edit` (`move_by`, `resize`, `rotate`, `move_points`): bound text follows its container, arrows follow
//! their targets, locked elements refuse. Each part can be turned off with `interaction` options.
//!
//! A drag does not accumulate small steps. The first update records the elements the transform touches; every
//! update puts them back and applies the transform for the total movement, so a resize that crosses the opposite
//! side and comes back ends exactly where it started, and Escape restores the original state.

use std::collections::BTreeSet;
use std::f64::consts::PI;

use drafftink_canvas::{Handle, Intent, MarqueeMode, Modifiers, Target};
use drafftink_edit::linear::{add_point, move_points};
use drafftink_edit::transform::{ResizeSpec, RotateMode, TransformError, TransformReport, move_by, resize, rotate};
use drafftink_geom::OrientedBox;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_geom::select::{BoxHandle, handle_point};
use drafftink_scene::MemoryTx;
use drafftink_scene::{Element, ElementId, GroupId, SceneRead, SceneStore, SceneTx, StoreError};

use crate::core::Core;
use crate::error::ApiError;
use crate::options::MarqueeOption;

/// A rotation drag with Shift snaps to this step.
const ROTATE_SNAP: f64 = PI / 12.0;

/// The drag in progress.
#[derive(Debug)]
pub(crate) enum Gesture {
    /// Moving elements.
    Move { ids: Vec<ElementId>, originals: Option<Vec<Element>> },
    /// Dragging a handle of the selection box.
    Resize { handle: Handle, ids: Vec<ElementId>, start: Option<OrientedBox>, originals: Option<Vec<Element>> },
    /// Dragging a point of a line or arrow.
    Point { element: ElementId, index: usize, original: Element },
    /// Dragging the middle of a segment, which first inserts a point.
    Midpoint { element: ElementId, segment: usize, original: Element },
    /// Drawing the rubber band; `base` stays selected (Shift adds to the selection).
    Marquee { base: Vec<ElementId> },
}

impl Core {
    /// The elements a click on `id` selects as a unit: the members of `group`, or the element.
    fn unit_members(&self, id: &ElementId, group: Option<&GroupId>) -> Vec<ElementId> {
        let store = self.canvas.store();
        match group {
            Some(g) => {
                let mut members: Vec<ElementId> = store.derived().group_members(g).cloned().collect();
                members.sort();
                members
            }
            None => vec![id.clone()],
        }
    }

    /// Replaces each id with the members of its outermost group.
    fn expand_groups(&self, ids: Vec<ElementId>) -> Vec<ElementId> {
        let store = self.canvas.store();
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for id in ids {
            let outer = store.get(&id).and_then(|e| e.group_ids.last().cloned());
            let members = match outer.filter(|g| store.derived().is_group(g)) {
                Some(g) => self.unit_members(&id, Some(&g)),
                None => vec![id],
            };
            for m in members {
                if seen.insert(m.clone()) {
                    out.push(m);
                }
            }
        }
        out
    }

    fn is_draggable(&self, id: &ElementId) -> bool {
        let custom = self.canvas.store().get(id).and_then(|e| e.custom.as_ref());
        custom.and_then(|c| c.get("draggable")).and_then(|v| v.as_bool()) != Some(false)
    }

    /// Acts on the intents of one input event.
    pub(crate) fn interact(&mut self, intents: &[Intent]) {
        for intent in intents {
            let result = match intent {
                Intent::Press { target, meta, .. } => self.on_press(target, meta.modifiers),
                Intent::DragStart { target, meta, .. } => self.on_drag_start(target, meta.modifiers),
                Intent::DragUpdate { origin, at, meta, .. } => self.on_drag(origin.world, at.world, meta.modifiers),
                Intent::DragEnd { origin, at, cancelled, meta, .. } => {
                    let last = if *cancelled { Ok(()) } else { self.on_drag(origin.world, at.world, meta.modifiers) };
                    last.and_then(|()| self.on_drag_end(origin.world, at.world, *cancelled))
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                // A refusal (a locked element, an invalid result) ends the gesture and leaves the scene as it was.
                log::warn!("{e}");
                self.abort_gesture();
            }
        }
    }

    fn on_press(&mut self, target: &Target, mods: Modifiers) -> Result<(), ApiError> {
        if !self.interaction.select {
            return Ok(());
        }
        match target {
            Target::Element { id, unit_group } => {
                let members = self.unit_members(id, unit_group.as_ref());
                let mut selection = self.canvas.selection().to_vec();
                if mods.shift {
                    if members.iter().all(|m| selection.contains(m)) {
                        selection.retain(|s| !members.contains(s));
                    } else {
                        let added: Vec<ElementId> = members.into_iter().filter(|m| !selection.contains(m)).collect();
                        selection.extend(added);
                    }
                    self.set_selection_ids(selection);
                } else if !selection.contains(id) {
                    self.set_selection_ids(members);
                }
            }
            Target::Empty if !mods.shift => self.set_selection_ids(Vec::new()),
            _ => {}
        }
        Ok(())
    }

    fn on_drag_start(&mut self, target: &Target, mods: Modifiers) -> Result<(), ApiError> {
        self.gesture = match target {
            Target::Handle(handle) if self.interaction.transform => self.start_handle(handle)?,
            Target::Element { id, unit_group } if self.interaction.drag && self.is_draggable(id) => {
                let selection = self.canvas.selection();
                let ids = if selection.contains(id) {
                    selection.to_vec()
                } else {
                    self.unit_members(id, unit_group.as_ref())
                };
                Some(Gesture::Move { ids, originals: None })
            }
            Target::Empty if self.interaction.marquee != MarqueeOption::Off => {
                let base = if mods.shift { self.canvas.selection().to_vec() } else { Vec::new() };
                Some(Gesture::Marquee { base })
            }
            _ => None,
        };
        Ok(())
    }

    fn start_handle(&self, handle: &Handle) -> Result<Option<Gesture>, ApiError> {
        let store = self.canvas.store();
        let original = |id: &ElementId| store.get(id).cloned().ok_or_else(|| ApiError::NotFound(id.clone()));
        Ok(Some(match handle {
            Handle::Point { element, index } => {
                Gesture::Point { element: element.clone(), index: *index, original: original(element)? }
            }
            Handle::Midpoint { element, segment } => {
                Gesture::Midpoint { element: element.clone(), segment: *segment, original: original(element)? }
            }
            box_handle => Gesture::Resize {
                handle: box_handle.clone(),
                ids: self.canvas.selection().to_vec(),
                start: self.canvas.selection_frame().obox(),
                originals: None,
            },
        }))
    }

    /// Applies the running gesture for the pointer at `at`.
    fn on_drag(&mut self, origin: Point, at: Point, mods: Modifiers) -> Result<(), ApiError> {
        let Some(mut gesture) = self.gesture.take() else { return Ok(()) };
        let result = self.advance(&mut gesture, origin, at, mods);
        self.gesture = Some(gesture);
        result
    }

    fn advance(&mut self, gesture: &mut Gesture, origin: Point, at: Point, mods: Modifiers) -> Result<(), ApiError> {
        let next = match gesture {
            Gesture::Move { ids, originals } => {
                let delta = at - origin;
                self.apply_total(ids, originals, |tx, ids| move_by(tx, ids, delta))?;
                None
            }
            Gesture::Resize { handle, ids, start, originals } => {
                self.drag_box_handle(handle, ids, start.as_ref(), originals, origin, at, mods)?;
                None
            }
            Gesture::Point { element, index, .. } => {
                self.run(|tx, _| Ok(move_points(tx, element, &[(*index, at)])?))?;
                None
            }
            Gesture::Midpoint { element, segment, original } => {
                let index = *segment + 1;
                self.run(|tx, _| Ok(add_point(tx, element, index, at)?))?;
                Some(Gesture::Point { element: element.clone(), index, original: original.clone() })
            }
            Gesture::Marquee { .. } => None,
        };
        if let Some(next) = next {
            *gesture = next;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn drag_box_handle(
        &mut self,
        handle: &Handle,
        ids: &[ElementId],
        start: Option<&OrientedBox>,
        originals: &mut Option<Vec<Element>>,
        origin: Point,
        at: Point,
        mods: Modifiers,
    ) -> Result<(), ApiError> {
        let (Some(box_handle), Some(start)) = (handle.to_box(), start) else { return Ok(()) };
        if box_handle == BoxHandle::Rotate {
            let turn = (at - start.center).atan2() - (origin - start.center).atan2();
            let snap = mods.shift.then_some(ROTATE_SNAP);
            self.apply_total(ids, originals, |tx, ids| rotate(tx, ids, turn, RotateMode::By, snap))
        } else {
            let delta = at - handle_point(start, box_handle, 0.0);
            let spec = ResizeSpec { handle: box_handle, delta, keep_aspect: mods.shift, from_center: mods.alt };
            self.apply_total(ids, originals, |tx, ids| resize(tx, ids, spec))
        }
    }

    /// Restores what the transform touched, then applies `op` for the total movement. See the module docs.
    fn apply_total(
        &mut self,
        ids: &[ElementId],
        originals: &mut Option<Vec<Element>>,
        op: impl Fn(&mut MemoryTx<'_>, &[ElementId]) -> Result<TransformReport, TransformError>,
    ) -> Result<(), ApiError> {
        if originals.is_none() {
            // A dry run in a transaction that is dropped tells which elements the transform touches (bound text,
            // frame children, re-routed arrows), so they can be put back later.
            let report = {
                let mut store = self.canvas.store_mut();
                let mut tx = store.begin();
                op(&mut tx, ids)?
            };
            let touched: BTreeSet<ElementId> = ids
                .iter()
                .cloned()
                .chain(report.changed)
                .chain(report.arrows)
                .chain(report.unbound.into_iter().map(|(id, _)| id))
                .collect();
            let store = self.canvas.store();
            *originals = Some(touched.iter().filter_map(|id| store.get(id).cloned()).collect());
        }
        let saved = originals.as_deref().unwrap_or_default();
        self.run(|tx, _| {
            restore(tx, saved)?;
            op(tx, ids)?;
            Ok(())
        })
    }

    fn on_drag_end(&mut self, origin: Point, at: Point, cancelled: bool) -> Result<(), ApiError> {
        let Some(gesture) = self.gesture.take() else { return Ok(()) };
        match gesture {
            Gesture::Move { originals: Some(saved), .. } | Gesture::Resize { originals: Some(saved), .. }
                if cancelled =>
            {
                self.restore_all(&saved)
            }
            Gesture::Point { original, .. } | Gesture::Midpoint { original, .. } if cancelled => {
                self.restore_all(&[original])
            }
            Gesture::Marquee { base } if !cancelled => {
                let mode = match self.interaction.marquee {
                    MarqueeOption::Intersect => MarqueeMode::Intersect,
                    _ => MarqueeMode::Contain,
                };
                let picked = self.expand_groups(self.canvas.marquee(&Rect::from_points(origin, at), mode));
                let mut selection = base;
                let added: Vec<ElementId> = picked.into_iter().filter(|id| !selection.contains(id)).collect();
                selection.extend(added);
                self.set_selection_ids(selection);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn restore_all(&mut self, saved: &[Element]) -> Result<(), ApiError> {
        self.run(|tx, _| Ok(restore(tx, saved)?))
    }

    /// Puts back what an interrupted gesture changed.
    fn abort_gesture(&mut self) {
        let saved = match self.gesture.take() {
            Some(Gesture::Move { originals: Some(saved), .. } | Gesture::Resize { originals: Some(saved), .. }) => {
                saved
            }
            Some(Gesture::Point { original, .. } | Gesture::Midpoint { original, .. }) => vec![original],
            _ => return,
        };
        if let Err(e) = self.restore_all(&saved) {
            log::warn!("could not undo the interrupted drag: {e}");
        }
    }
}

/// Writes saved elements back.
fn restore<T: SceneTx>(tx: &mut T, saved: &[Element]) -> Result<(), StoreError> {
    for original in saved {
        let value = original.clone();
        tx.update(&original.id, move |e| *e = value)?;
    }
    Ok(())
}
