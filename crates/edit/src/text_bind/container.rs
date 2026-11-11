//! Wrapping a free text in a new shape and binding it as the shape's label.

use drafftink_scene::{Element, ElementId, ElementKind, IdSource, Kind, SceneTx, Style, ZIndex};

use super::{BOUND_TEXT_PADDING, TextBindError, TextMeasure, bind_text, container_dim_for_text, live};
use crate::ids::unused_element_id;
use crate::target::TargetError;

/// The result of wrapping one text.
#[derive(Debug, Clone, PartialEq)]
pub struct Wrapped {
    /// The text, now a label.
    pub text: ElementId,
    /// The new shape.
    pub container: ElementId,
}

/// Wraps each free text in `ids` in a new shape of kind `shape` (rectangle, ellipse or diamond) with `padding`
/// around the text, and binds the text as its label. The shape sits directly below the text and takes its groups,
/// frame, colour and opacity. Texts that are already labels, and other kinds, are skipped; the second list holds them.
pub fn wrap_in_container<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids_src: &mut dyn IdSource,
    ids: &[ElementId],
    shape: Kind,
    padding: f64,
) -> Result<(Vec<Wrapped>, Vec<ElementId>), TextBindError> {
    let mut todo: Vec<ElementId> = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        let e = live(tx, id)?;
        match e.text_data() {
            Some(td) if td.container_id.is_none() => {
                if e.locked {
                    return Err(TextBindError::Target(TargetError::Locked(vec![id.clone()])));
                }
                if !todo.contains(id) {
                    todo.push(id.clone());
                }
            }
            _ => skipped.push(id.clone()),
        }
    }
    let pad = (padding - BOUND_TEXT_PADDING).max(0.0);
    let mut out = Vec::new();
    for tid in todo {
        let t = live(tx, &tid)?.clone();
        let (w, h) = (
            container_dim_for_text(t.transform.w + 2.0 * pad, shape),
            container_dim_for_text(t.transform.h + 2.0 * pad, shape),
        );
        let c = t.transform.center();
        let kind = match shape {
            Kind::Ellipse => ElementKind::Ellipse,
            Kind::Diamond => ElementKind::Diamond,
            _ => ElementKind::Rectangle,
        };
        let (below, _) = tx.z_neighbors(&tid);
        let z = ZIndex::between(below.as_ref(), Some(&t.index))?;
        let cid = unused_element_id(tx, ids_src);
        let mut style = Style::default();
        style.stroke_color = t.style.stroke_color;
        style.opacity = t.style.opacity;
        style.seed = ids_src.seed();
        let mut ce =
            Element::new(cid.clone(), kind, z).with_bounds(c.x - w / 2.0, c.y - h / 2.0, w, h).with_style(style);
        ce.transform.angle = t.transform.angle;
        ce.group_ids = t.group_ids.clone();
        ce.frame_id = t.frame_id.clone();
        tx.insert(ce)?;
        bind_text(tx, m, &tid, &cid)?;
        out.push(Wrapped { text: tid, container: cid });
    }
    Ok((out, skipped))
}
