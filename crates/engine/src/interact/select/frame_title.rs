//! The name label above a frame as the target of a double-click.
//!
//! The label is drawn in device space at a fixed size (`FrameData::LABEL_SIZE`), so its place is worked out on the
//! screen: from the frame's top-left corner, up by the room reserved for the label, as wide as the first line of the
//! name. A frame is hit by its outline only, so the label is no element: this module finds the frame whose label is
//! under the pointer.

use drafftink_canvas::Target;
use drafftink_geom::kurbo::{Point, Rect};
use drafftink_scene::{Element, ElementId, ElementKind, FontFamily, FontWeight, FrameData};

use crate::interact::ctx::Ctx;
use crate::interact::events::Ev;

/// Logical pixels added at the right of the text, so that the end of a name is easy to hit.
const SLACK_PX: f64 = 8.0;

/// The label of a frame, as the canvas shows it: the first line of the name, or the default.
fn shown_label(data: &FrameData) -> &str {
    data.name.as_deref().unwrap_or(FrameData::DEFAULT_NAME).lines().next().unwrap_or("")
}

/// The rectangle of the label of `frame` on the screen, in logical pixels. `None` for a frame whose label is blank
/// (the canvas draws nothing then).
fn label_rect(cx: &Ctx<'_>, frame: &Element, data: &FrameData) -> Option<Rect> {
    let label = shown_label(data);
    if label.trim().is_empty() {
        return None;
    }
    let corner = cx.world.viewport.world_to_screen(Point::new(frame.transform.x, frame.transform.y));
    let (text_width, _) =
        cx.s.services.io().text_size(label, FontFamily::GelPen, FontWeight::Regular, FrameData::LABEL_SIZE, 1.0);
    // The canvas cuts the label at the width of the frame on screen, but never narrower than the minimum.
    let clip = cx.world.viewport.world_to_px(frame.transform.w.max(0.0)).max(FrameData::LABEL_MIN_WIDTH);
    let width = (text_width + SLACK_PX).min(clip);
    Some(Rect::new(corner.x, corner.y - FrameData::LABEL_ROOM, corner.x + width, corner.y))
}

/// The frame whose name label lies under the pointer, with the label of that frame. A pointer that is over some
/// other element, a handle or a link icon is not on a label: the label gives way to what lies over it. The topmost
/// frame wins where labels overlap.
pub(super) fn frame_at(cx: &Ctx<'_>, ev: &Ev<'_>) -> Option<(ElementId, String)> {
    let outline_of_a_frame = match ev.target {
        Target::Empty => true,
        Target::Element { id, .. } => cx.s.element(id).is_some_and(|e| matches!(e.kind, ElementKind::Frame(_))),
        Target::None | Target::Handle(_) | Target::LinkIcon { .. } => false,
    };
    if !outline_of_a_frame {
        return None;
    }
    let reach = cx.world.px(FrameData::LABEL_ROOM);
    cx.world
        .near(cx.s, ev.at.world, reach)
        .iter()
        .filter_map(|id| cx.s.element(id))
        .filter_map(|e| match &e.kind {
            ElementKind::Frame(data) if label_rect(cx, e, data).is_some_and(|r| r.contains(ev.at.screen)) => {
                Some((e, data))
            }
            _ => None,
        })
        .max_by(|(a, _), (b, _)| a.index.cmp(&b.index))
        .map(|(e, data)| (e.id.clone(), data.name.clone().unwrap_or_else(|| FrameData::DEFAULT_NAME.to_owned())))
}
