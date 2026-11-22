//! The highlighter (E9): a drag turns the highlight on, or off, for what the path touches.
//!
//! The tool creates no element. While the pointer is down the path, the elements it touches and the characters
//! of the texts it crosses are only shown (a thin trail, outlines, bands over the characters). At the release one
//! `highlight.apply` writes all of it, so the whole drag is one undo step. The colour is the tool's marker colour
//! (`tool.set_marker`); with "none" selected, or with Alt held, the drag removes the highlight instead.
//!
//! Elements are found like the eraser finds them (the pointer within a few pixels of the shape). A text is not
//! found by its glyphs but by its box, and the characters are the ones the path crosses on the real layout
//! (`drafftink_edit::highlight::CharSweep`), so the spaces between words are highlighted as well.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use drafftink_edit::highlight::{CharSweep, byte_range_to_chars};
use drafftink_edit::style::{Prop, applies};
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_scene::{Element, ElementId, ElementKind, MarkerColor, SceneRead};
use drafftink_typeset::{TextLayout, TextSelection};
use serde_json::json;

use crate::interact::ctx::Ctx;
use crate::interact::events::Ev;
use crate::interact::overlay::MarkerPreview;

/// How far from the path an element counts as touched, in screen pixels.
const REACH_PX: f64 = 6.0;
/// A path never holds more points than this.
const MAX_PATH: usize = 8_000;
/// One pointer step is probed at most this many times, however far the pointer jumped.
const MAX_PROBES: usize = 400;

/// A text the path has come near: its layout and the characters crossed so far.
#[derive(Debug)]
struct TextHits {
    layout: Arc<TextLayout>,
    content: String,
    sweep: CharSweep,
    /// The text was reached by the latest probe.
    reached: bool,
}

/// The drag of the highlighter.
#[derive(Debug, Default)]
pub(super) struct Marker {
    path: Vec<Point>,
    /// Elements other than texts that the path touched.
    whole: BTreeSet<ElementId>,
    texts: BTreeMap<ElementId, TextHits>,
}

impl Marker {
    /// Extends the path to the pointer and shows what it touches.
    pub(super) fn update(&mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.begin(cx, ev);
        self.extend(cx, ev.at.world);
        self.show(cx);
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
        let step = cx.px(REACH_PX / 2.0).max(f64::EPSILON);
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

    /// Adds what the marker touches at `p`.
    fn probe(&mut self, cx: &Ctx<'_>, p: Point) {
        let reach = cx.px(REACH_PX);
        self.texts.values_mut().for_each(|t| t.reached = false);
        let zone = Rect::from_center_size(p, (reach * 2.0, reach * 2.0));
        let near: Vec<&Element> = cx.s.tab().doc.scene_index().in_rect(cx.s.store(), &zone);
        for e in near.into_iter().filter(|e| !e.deleted && !e.locked) {
            let ElementKind::Text(data) = &e.kind else { continue };
            if e.transform.obox().distance_to_point(p) > reach {
                continue;
            }
            let hits = match self.texts.entry(e.id.clone()) {
                std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::btree_map::Entry::Vacant(v) => {
                    let Some(layout) = cx.s.services.text_layout(data, e.transform.w) else { continue };
                    v.insert(TextHits {
                        layout,
                        content: data.content.clone(),
                        sweep: CharSweep::new(),
                        reached: false,
                    })
                }
            };
            hits.sweep.probe(&hits.layout, e.transform.world_to_local(p), reach);
            hits.reached = true;
        }
        self.texts.values_mut().filter(|t| !t.reached).for_each(|t| t.sweep.lift());
        for hit in cx.world.hit_all(cx.s, p, REACH_PX) {
            let Some(e) = cx.s.store().get(&hit.id) else { continue };
            if !matches!(e.kind, ElementKind::Text(_)) && applies(e, Prop::Highlight) {
                self.whole.insert(hit.id);
            }
        }
    }

    /// The colour of this drag: the marker's, or none while Alt is held.
    fn color(cx: &Ctx<'_>) -> Option<MarkerColor> {
        if cx.mods.alt { None } else { cx.s.tool.marker }
    }

    /// Writes the trail, the outlines and the bands into the overlay parts.
    fn show(&self, cx: &mut Ctx<'_>) {
        let bands = self
            .texts
            .iter()
            .filter_map(|(id, t)| Some((cx.s.store().get(id)?, t)))
            .flat_map(|(e, t)| {
                let to_world = e.transform.frame().to_world();
                let sweep = &t.sweep;
                sweep
                    .byte_ranges(&t.content)
                    .into_iter()
                    .flat_map(|r| t.layout.selection_rects(&selection(&t.layout, r.start, r.end)))
                    .map(move |r| corners(r).map(|c| to_world * c))
            })
            .collect();
        let ids: Vec<ElementId> = self.whole.iter().cloned().collect();
        if cx.parts.highlight != ids {
            cx.parts.highlight = ids;
        }
        cx.parts.marker = Some(MarkerPreview { trail: self.path.clone(), bands, color: Self::color(cx) });
        cx.overlay_changed();
    }

    /// The arguments of the `highlight.apply` that ends the drag, or `None` when the path touched nothing: the
    /// whole elements as `ids`, and the crossed characters of each text as `ranges` (character indices).
    fn apply_args(&self, color: Option<MarkerColor>) -> Option<serde_json::Value> {
        let ids: Vec<&str> = self.whole.iter().map(ElementId::as_str).collect();
        let ranges: Vec<serde_json::Value> = self
            .texts
            .iter()
            .flat_map(|(id, t)| {
                t.sweep.byte_ranges(&t.content).into_iter().map(move |r| {
                    let chars = byte_range_to_chars(&t.content, &r);
                    json!({ "id": id.as_str(), "start": chars.start, "end": chars.end })
                })
            })
            .collect();
        if ids.is_empty() && ranges.is_empty() {
            return None;
        }
        let mut args = json!({ "color": color, "ranges": ranges });
        // Without `ids` the command acts on the selection when there are no ranges: only name them when there are some.
        if !ids.is_empty() {
            args["ids"] = json!(ids);
        }
        Some(args)
    }

    /// The pointer was released: one `highlight.apply` for the whole drag.
    pub(super) fn finish(mut self, cx: &mut Ctx<'_>, ev: &Ev<'_>) {
        self.begin(cx, ev);
        self.extend(cx, ev.at.world);
        if let Some(args) = self.apply_args(Self::color(cx)) {
            cx.exec("highlight.apply", args);
        }
    }
}

fn selection(layout: &TextLayout, start: usize, end: usize) -> TextSelection {
    layout.selection_from_range(start, end)
}

/// The corners of a rectangle, clockwise from the top left.
fn corners(r: Rect) -> [Point; 4] {
    [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1)]
}
