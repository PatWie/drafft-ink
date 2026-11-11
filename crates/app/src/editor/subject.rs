//! What a text edit is about and where its text goes: a free text, or the label of a shape or an arrow.
//!
//! The editor draws the text the way the committed element is drawn. It builds the element that
//! `text.edit_end` would leave behind (same font, same wrap, same position rules, `edit::text_bind::fit_label`
//! for labels) and hands it to the renderer. The caret and the selection are read from the same layout.

use std::sync::Arc;

use drafftink_edit::text_bind::{GrowAnchor, TextProps, fit_label, label_of, new_label_font};
use drafftink_edit::text_edit::Shown;
use drafftink_engine::Session;
use drafftink_engine::interact::UiRequest;
use drafftink_geom::kurbo::{Affine, Point, Size};
use drafftink_render::{element_to_world, text_spec};
use drafftink_scene::{Element, ElementId, ElementKind, InlineMath, Kind, SceneRead, TextData, TextRun, ZIndex};
use drafftink_typeset::{TextLayout, Typesetter, Wrap};

/// Where the text goes.
#[derive(Debug, Clone)]
enum Place {
    /// A free text. The box follows the text, or has the fixed width of a dragged-out box.
    Free { width: Option<f64> },
    /// The label of a shape or an arrow.
    Label { container: Element },
}

/// What is being edited.
#[derive(Debug, Clone)]
pub(super) struct Subject {
    place: Place,
    /// The text element as it is now, or as it will be created.
    template: Element,
    /// The text element that exists in the document (a label that is not made yet, and a draft, have none).
    existing: Option<ElementId>,
}

/// The content an edit starts with.
#[derive(Debug, Clone)]
pub(super) struct Initial {
    pub(super) text: String,
    pub(super) runs: Vec<TextRun>,
    pub(super) math: InlineMath,
}

/// The text as it is drawn now.
#[derive(Debug, Clone)]
pub(crate) struct Arranged {
    /// The text element the renderer draws (not in the document).
    pub(crate) text: Element,
    /// The container with the size it grows to, when the text no longer fits in it.
    pub(crate) grown_container: Option<Element>,
    /// The layout of the text, as the renderer lays it out (with the formula under the caret open).
    pub(crate) layout: Arc<TextLayout>,
    /// `TextSpec::reveal` of that layout.
    pub(crate) reveal: Option<usize>,
    /// Layout space of the text to world space.
    pub(crate) to_world: Affine,
}

/// Id of the element that stands for text that is not in the document yet. It is never stored.
fn draft_id() -> Option<ElementId> {
    ElementId::new("text-edit-draft").ok()
}

impl Subject {
    /// The subject a request names, with the content the edit starts from. `None` when the element is gone or
    /// cannot hold text.
    pub(super) fn open(s: &Session, request: &UiRequest) -> Option<(Self, Initial)> {
        let UiRequest::OpenTextEditor { element, at, width } = request else { return None };
        let store = s.store();
        let Some(id) = element else {
            let mut data = TextData::new("");
            data.font = s.tool.text_defaults.clone();
            data.auto_resize = width.is_none();
            let mut e = Element::new(draft_id()?, ElementKind::Text(data), ZIndex::first());
            e.style.stroke_color = s.tool.style_defaults.stroke_color;
            e.style.opacity = s.tool.style_defaults.opacity;
            e.transform.x = at.x;
            e.transform.y = at.y;
            e.transform.w = width.unwrap_or(0.0);
            let initial = Initial { text: String::new(), runs: Vec::new(), math: data_math(&e) };
            return Some((Self { place: Place::Free { width: *width }, template: e, existing: None }, initial));
        };
        let el = store.get(id)?;
        match &el.kind {
            ElementKind::Text(t) => {
                let place = match t.container_id.as_ref().and_then(|c| store.get(c)) {
                    Some(c) => Place::Label { container: c.clone() },
                    None => Place::Free { width: (!t.auto_resize).then_some(el.transform.w) },
                };
                let initial = Initial { text: t.content.clone(), runs: t.runs.clone(), math: t.font.inline_math };
                Some((Self { place, template: el.clone(), existing: Some(id.clone()) }, initial))
            }
            kind if el.tag().is_label_container() => {
                let _ = kind;
                match label_of(store, id).and_then(|l| store.get(&l)) {
                    Some(label) => {
                        let ElementKind::Text(t) = &label.kind else { return None };
                        let initial =
                            Initial { text: t.content.clone(), runs: t.runs.clone(), math: t.font.inline_math };
                        let subject = Self {
                            place: Place::Label { container: el.clone() },
                            template: label.clone(),
                            existing: Some(label.id.clone()),
                        };
                        Some((subject, initial))
                    }
                    None => Some(Self::new_label(s, el)?),
                }
            }
            _ => None,
        }
    }

    /// A label that does not exist yet: centred, in the tool's font, in the colour of its container.
    fn new_label(s: &Session, container: &Element) -> Option<(Self, Initial)> {
        let mut data = TextData::new("");
        data.font = new_label_font(Some(&s.tool.text_defaults));
        data.container_id = Some(container.id.clone());
        data.label_position = (container.tag() == Kind::Arrow).then_some(0.5);
        let mut e = Element::new(draft_id()?, ElementKind::Text(data), ZIndex::first());
        e.style.stroke_color = container.style.stroke_color;
        e.style.opacity = container.style.opacity;
        let initial = Initial { text: String::new(), runs: Vec::new(), math: data_math(&e) };
        Some((Self { place: Place::Label { container: container.clone() }, template: e, existing: None }, initial))
    }

    /// The document elements to leave out of the scene while the editor draws them: the text, and the container
    /// when it grows.
    pub(super) fn hidden(&self, grown: bool) -> Vec<ElementId> {
        let mut out: Vec<ElementId> = self.existing.iter().cloned().collect();
        if let (true, Place::Label { container }) = (grown, &self.place) {
            out.push(container.id.clone());
        }
        out
    }

    /// Lays the shown text out in its place.
    pub(super) fn arrange(&self, shown: &Shown, math: InlineMath, props: &TextProps, ts: &mut Typesetter) -> Arranged {
        let mut text = self.template.clone();
        text.style.stroke_color = props.stroke_color.unwrap_or(text.style.stroke_color);
        text.style.opacity = props.opacity.unwrap_or(text.style.opacity);
        let mut data = match &self.template.kind {
            ElementKind::Text(t) => t.clone(),
            _ => TextData::new(""),
        };
        props.apply_font(&mut data.font);
        data.font.inline_math = math;
        data.content.clone_from(&shown.text);
        data.runs = shown.text_runs();
        let stroke = text.style.stroke_color;
        let layout_at = |data: &TextData, width: f64, ts: &mut Typesetter| {
            let mut spec = text_spec(data, stroke, width);
            spec.reveal = shown.reveal;
            ts.text(&spec)
        };
        let mut grown_container = None;
        match &self.place {
            Place::Free { width } => {
                data.auto_resize = width.is_none();
                let layout = layout_at(&data, width.unwrap_or(0.0), ts);
                text.transform.w = width.unwrap_or(f64::from(layout.size.width));
                text.transform.h = f64::from(layout.size.height);
            }
            Place::Label { container } => {
                data.auto_resize = true;
                let font = data.font.clone();
                let fit = fit_label(container, &font, data.label_position, GrowAnchor::Top, &mut |width| {
                    let mut spec = text_spec(&data, stroke, width);
                    spec.wrap = Wrap::Width(width as f32);
                    spec.reveal = shown.reveal;
                    let size = ts.text(&spec).size;
                    Size::new(f64::from(size.width), f64::from(size.height))
                });
                if let Ok(fit) = fit {
                    text.transform = fit.text;
                    if fit.grew {
                        let mut c = container.clone();
                        c.transform.x = fit.container.x0;
                        c.transform.y = fit.container.y0;
                        c.transform.w = fit.container.width();
                        c.transform.h = fit.container.height();
                        grown_container = Some(c);
                    }
                }
            }
        }
        let layout = layout_at(&data, text.transform.w, ts);
        text.kind = ElementKind::Text(data);
        let to_world = element_to_world(&text);
        Arranged { text, grown_container, layout, reveal: shown.reveal, to_world }
    }
}

impl Arranged {
    /// A point in world space as a point of the layout.
    pub(crate) fn to_local(&self, world: Point) -> Point {
        self.to_world.inverse() * world
    }
}

fn data_math(e: &Element) -> InlineMath {
    match &e.kind {
        ElementKind::Text(t) => t.font.inline_math,
        _ => InlineMath::Auto,
    }
}
