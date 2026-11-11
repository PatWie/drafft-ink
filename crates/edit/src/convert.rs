//! Changing the type of an element in place (`element.convert`): the rules of Excalidraw's "convert element type".
//!
//! Two families, and an element never leaves its family:
//!
//! * **Shapes**: rectangle, diamond, ellipse. The element keeps its id, box, angle, style, groups, frame, bound
//!   label and the arrows bound to it. The label is laid out again for the new inner box (a diamond or ellipse holds
//!   less text than a rectangle, so the shape grows about its centre when it must), arrows that sat on the old
//!   outline are put back on the new one ([`crate::binding::reanchor_to_outline`]) and every bound arrow is solved.
//! * **Linear**: line, sharp arrow, round arrow, elbow arrow ([`Target`]). The points stay. A line gets an end head
//!   when it becomes an arrow and loses its heads when it becomes a line. A path change goes through
//!   [`crate::linear::set_path`], so the elbow router and the bindings follow their own rules.
//!
//! Not convertible: text, math, images, frames and freehand strokes; a selection that mixes the two families; a
//! locked element. An arrow that is bound to a shape or carries a label cannot become a line (a line holds
//! neither), and a closed line is an outline, not an arrow.
//!
//! [`cycle`] gives the target of the Tab shortcut: the next type in the family's order, as in Excalidraw.
//!
//! Adapted from Excalidraw (MIT), `components/ConvertElementTypePopup.tsx`; see `ATTRIBUTION.md`.
//!
//! Callers run these functions inside one transaction.

use drafftink_scene::{
    Element, ElementId, ElementKind, Head, Kind, LinearData, PathType, SceneRead, SceneTx, StoreError,
};

use crate::binding::{self, BindingError};
use crate::linear::{self, LinearError};
use crate::text_bind::{self, GrowAnchor, TextBindError, TextMeasure};

/// The types an element can be converted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A rectangle.
    Rectangle,
    /// A diamond.
    Diamond,
    /// An ellipse.
    Ellipse,
    /// A line, sharp or round (a round line stays round).
    Line,
    /// An arrow of the given path type.
    Arrow(PathType),
}

/// The type families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Rectangle, diamond, ellipse.
    Shape,
    /// Line and arrows.
    Linear,
}

/// The order of Tab in the shape family.
const SHAPE_ORDER: [Target; 3] = [Target::Rectangle, Target::Diamond, Target::Ellipse];
/// The order of Tab in the linear family.
const LINEAR_ORDER: [Target; 4] =
    [Target::Line, Target::Arrow(PathType::Sharp), Target::Arrow(PathType::Round), Target::Arrow(PathType::Elbow)];

impl Target {
    /// The family of this type.
    pub fn family(self) -> Family {
        match self {
            Self::Rectangle | Self::Diamond | Self::Ellipse => Family::Shape,
            Self::Line | Self::Arrow(_) => Family::Linear,
        }
    }

    /// Plain words for messages: "rectangle", "sharp arrow", "elbow arrow", "line".
    pub fn words(self) -> &'static str {
        match self {
            Self::Rectangle => "rectangle",
            Self::Diamond => "diamond",
            Self::Ellipse => "ellipse",
            Self::Line => "line",
            Self::Arrow(PathType::Sharp) => "sharp arrow",
            Self::Arrow(PathType::Round) => "round arrow",
            Self::Arrow(PathType::Elbow) => "elbow arrow",
        }
    }

    /// The type of an element, `None` when it is not convertible. A line is `Line` whatever its path.
    pub fn of(e: &Element) -> Option<Target> {
        match &e.kind {
            ElementKind::Rectangle => Some(Self::Rectangle),
            ElementKind::Diamond => Some(Self::Diamond),
            ElementKind::Ellipse => Some(Self::Ellipse),
            ElementKind::Line(_) => Some(Self::Line),
            ElementKind::Arrow(d) => Some(Self::Arrow(d.path)),
            _ => None,
        }
    }

    fn order(family: Family) -> &'static [Target] {
        match family {
            Family::Shape => &SHAPE_ORDER,
            Family::Linear => &LINEAR_ORDER,
        }
    }
}

/// The target of Tab (`reverse` false) or Shift+Tab (`reverse` true) for elements of one `family` with the given
/// current types: one step along the family's order from the type they all share, and from before the first
/// type (so forwards gives the first, backwards the last but one, as Excalidraw does) when they differ.
pub fn cycle(family: Family, current: &[Target], reverse: bool) -> Target {
    let order = Target::order(family);
    let common = current.first().filter(|first| current.iter().all(|t| t == *first));
    let at = common.and_then(|t| order.iter().position(|o| o == t));
    let len = order.len();
    // Index arithmetic in `usize`: `at + 1` going forward, `at + len - 1` going back, both modulo `len`. With no
    // common type the start is "before the first", `len - 1` going forward after the modulo.
    let from = at.unwrap_or(len - 1);
    let next = if reverse { (from + len - 1) % len } else { (from + 1) % len };
    order.get(next).copied().unwrap_or(Target::Rectangle)
}

/// Why a conversion was refused. Nothing is changed when it is returned.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConvertError {
    /// There is nothing to convert.
    #[error("no element was given")]
    NoElements,
    /// An element does not exist or is deleted.
    #[error("element {0} does not exist or is deleted")]
    NotFound(ElementId),
    /// Elements are locked.
    #[error("{} element(s) are locked", .0.len())]
    Locked(Vec<ElementId>),
    /// The kind has no other types: text, math, image, frame, freehand.
    #[error("{id} is {kind}, which has no other type")]
    NotConvertible {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
    },
    /// The elements belong to both families.
    #[error("the elements mix shapes and lines or arrows")]
    Mixed,
    /// The target is in the other family from the elements.
    #[error("{id} is {from} and cannot become {}", to.words())]
    WrongFamily {
        /// The first element that does not fit.
        id: ElementId,
        /// Its current type, in words.
        from: &'static str,
        /// The target.
        to: Target,
    },
    /// An arrow bound to a shape cannot become a line.
    #[error("the arrow {0} is bound to a shape and cannot become a line")]
    BoundArrow(ElementId),
    /// An arrow with a label cannot become a line.
    #[error("the arrow {0} has a label and cannot become a line")]
    LabelledArrow(ElementId),
    /// A closed line is an outline and cannot become an arrow.
    #[error("the line {0} is closed and cannot become an arrow")]
    ClosedLine(ElementId),
    /// Every element already has the target type.
    #[error("every element is already {}", to.words())]
    AlreadyThere {
        /// The target.
        to: Target,
    },
    /// The label could not be laid out.
    #[error(transparent)]
    Label(#[from] TextBindError),
    /// The path change failed.
    #[error(transparent)]
    Linear(#[from] LinearError),
    /// Solving the bound arrows failed.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// The store refused.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What a conversion did.
#[derive(Debug, Clone, PartialEq)]
pub struct Converted {
    /// The type the elements have now.
    pub to: Target,
    /// The elements whose type changed, in the order given.
    pub changed: Vec<ElementId>,
    /// Elements already of that type, left alone.
    pub unchanged: Vec<ElementId>,
    /// Arrows that followed a converted shape (their ends were put back on the new outline).
    pub followers: Vec<ElementId>,
}

/// The elements to convert, checked: live, unlocked, all convertible and all of one family.
fn gather<R: SceneRead>(r: &R, ids: &[ElementId]) -> Result<(Family, Vec<Element>), ConvertError> {
    if ids.is_empty() {
        return Err(ConvertError::NoElements);
    }
    let mut elements = Vec::with_capacity(ids.len());
    for id in ids {
        let e = r.get(id).filter(|e| !e.deleted).ok_or_else(|| ConvertError::NotFound(id.clone()))?;
        elements.push(e.clone());
    }
    let locked: Vec<ElementId> = elements.iter().filter(|e| e.locked).map(|e| e.id.clone()).collect();
    if !locked.is_empty() {
        return Err(ConvertError::Locked(locked));
    }
    let mut family = None;
    for e in &elements {
        let target =
            Target::of(e).ok_or_else(|| ConvertError::NotConvertible { id: e.id.clone(), kind: e.tag().name() })?;
        match family {
            None => family = Some(target.family()),
            Some(f) if f != target.family() => return Err(ConvertError::Mixed),
            Some(_) => {}
        }
    }
    let family = family.ok_or(ConvertError::NoElements)?;
    Ok((family, elements))
}

/// Changes the type of the elements `ids`. `to` names the target; `None` takes the next type of the family
/// ([`cycle`], backwards when `reverse`).
pub fn convert<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &[ElementId],
    to: Option<Target>,
    reverse: bool,
) -> Result<Converted, ConvertError> {
    let (family, elements) = gather(&*tx, ids)?;
    let current: Vec<Target> = elements.iter().filter_map(Target::of).collect();
    let to = to.unwrap_or_else(|| cycle(family, &current, reverse));
    if let Some(first) = elements.first().filter(|_| to.family() != family) {
        let from = Target::of(first).map_or("this element", Target::words);
        return Err(ConvertError::WrongFamily { id: first.id.clone(), from, to });
    }
    let (todo, unchanged): (Vec<&Element>, Vec<&Element>) = elements.iter().partition(|e| Target::of(e) != Some(to));
    if todo.is_empty() {
        return Err(ConvertError::AlreadyThere { to });
    }
    if to == Target::Line {
        for e in &todo {
            check_becomes_line(&*tx, e)?;
        }
    }
    if matches!(to, Target::Arrow(_)) {
        for e in &todo {
            if e.linear_data().is_some_and(|d| d.closed) {
                return Err(ConvertError::ClosedLine(e.id.clone()));
            }
        }
    }
    let changed: Vec<ElementId> = todo.iter().map(|e| e.id.clone()).collect();
    let mut followers = Vec::new();
    match to {
        Target::Rectangle | Target::Diamond | Target::Ellipse => followers = convert_shapes(tx, m, &changed, to)?,
        Target::Line | Target::Arrow(_) => {
            for id in &changed {
                convert_linear(tx, id, to)?;
            }
        }
    }
    let unchanged = unchanged.iter().map(|e| e.id.clone()).collect();
    Ok(Converted { to, changed, unchanged, followers })
}

/// An arrow can become a line when no shape holds an end and no label sits on it.
fn check_becomes_line<R: SceneRead>(r: &R, e: &Element) -> Result<(), ConvertError> {
    if e.tag() != Kind::Arrow {
        return Ok(());
    }
    if e.binding_targets().iter().any(Option::is_some) {
        return Err(ConvertError::BoundArrow(e.id.clone()));
    }
    if text_bind::label_of(r, &e.id).is_some() {
        return Err(ConvertError::LabelledArrow(e.id.clone()));
    }
    Ok(())
}

fn shape_kind(to: Target) -> Option<ElementKind> {
    match to {
        Target::Rectangle => Some(ElementKind::Rectangle),
        Target::Diamond => Some(ElementKind::Diamond),
        Target::Ellipse => Some(ElementKind::Ellipse),
        Target::Line | Target::Arrow(_) => None,
    }
}

/// Rectangle, diamond or ellipse: new kind, label laid out again, bindings put back on the outline, arrows solved.
fn convert_shapes<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &[ElementId],
    to: Target,
) -> Result<Vec<ElementId>, ConvertError> {
    let Some(kind) = shape_kind(to) else { return Ok(Vec::new()) };
    // The arrows must be found before anything is written: a transaction that has written no longer offers the
    // committed index.
    let candidates = binding::arrows_bound_to(&*tx, ids);
    for id in ids {
        tx.update(id, |e| e.kind = kind.clone())?;
        text_bind::relayout_label(tx, m, id, GrowAnchor::Center)?;
    }
    binding::reanchor_to_outline(tx, ids)?;
    let followers = binding::update_bound_arrows_from(tx, ids, candidates)?;
    text_bind::refresh_labels(tx, m, &followers, GrowAnchor::Top)?;
    Ok(followers)
}

/// Line or arrow to line or arrow.
fn convert_linear<T: SceneTx>(tx: &mut T, id: &ElementId, to: Target) -> Result<(), ConvertError> {
    match to {
        Target::Line => {
            // A line is never an elbow: leave the elbow first, then change the kind.
            if tx.get(id).and_then(|e| e.linear_data()).is_some_and(|d| d.path == PathType::Elbow) {
                linear::set_path(tx, id, PathType::Sharp)?;
            }
            tx.update(id, |e| {
                if let ElementKind::Arrow(d) = &mut e.kind {
                    let mut line: LinearData = std::mem::replace(d, LinearData::new(Vec::new()));
                    line.start_head = Head::None;
                    line.end_head = Head::None;
                    e.kind = ElementKind::Line(line);
                }
            })?;
        }
        Target::Arrow(path) => {
            tx.update(id, |e| {
                if let ElementKind::Line(d) = &mut e.kind {
                    let mut arrow: LinearData = std::mem::replace(d, LinearData::new(Vec::new()));
                    arrow.end_head = crate::create::DEFAULT_ARROW_END_HEAD;
                    e.kind = ElementKind::Arrow(arrow);
                }
            })?;
            linear::set_path(tx, id, path)?;
        }
        Target::Rectangle | Target::Diamond | Target::Ellipse => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use drafftink_scene::{MemoryStore, SceneStore, Transform, ZIndex};

    use super::*;
    use crate::text_bind::MonospaceMeasure;

    const L: Target = Target::Line;
    const SHARP: Target = Target::Arrow(PathType::Sharp);
    const ROUND: Target = Target::Arrow(PathType::Round);
    const ELBOW: Target = Target::Arrow(PathType::Elbow);

    #[test]
    fn tab_walks_the_shapes_in_order_and_wraps() {
        assert_eq!(cycle(Family::Shape, &[Target::Rectangle], false), Target::Diamond);
        assert_eq!(cycle(Family::Shape, &[Target::Diamond], false), Target::Ellipse);
        assert_eq!(cycle(Family::Shape, &[Target::Ellipse], false), Target::Rectangle);
        assert_eq!(cycle(Family::Shape, &[Target::Rectangle], true), Target::Ellipse);
    }

    #[test]
    fn tab_walks_the_linear_types_in_order_and_wraps() {
        assert_eq!(cycle(Family::Linear, &[L], false), SHARP);
        assert_eq!(cycle(Family::Linear, &[SHARP], false), ROUND);
        assert_eq!(cycle(Family::Linear, &[ROUND], false), ELBOW);
        assert_eq!(cycle(Family::Linear, &[ELBOW], false), L);
        assert_eq!(cycle(Family::Linear, &[L], true), ELBOW);
    }

    #[test]
    fn mixed_current_types_start_from_before_the_first() {
        assert_eq!(cycle(Family::Shape, &[Target::Rectangle, Target::Ellipse], false), Target::Rectangle);
        assert_eq!(cycle(Family::Linear, &[L, ELBOW], false), L);
    }

    fn add(tx: &mut impl SceneTx, id: &str, kind: ElementKind) -> ElementId {
        let id = ElementId::new(id).unwrap();
        let z: ZIndex = tx.next_top_index().unwrap();
        let mut e = Element::new(id.clone(), kind, z);
        e.transform = Transform::new(0.0, 0.0, 100.0, 60.0);
        tx.insert(e).unwrap();
        id
    }

    fn ids(list: &[&ElementId]) -> Vec<ElementId> {
        list.iter().map(|i| (*i).clone()).collect()
    }

    #[test]
    fn the_errors_name_the_first_problem_and_leave_the_scene_alone() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let m = MonospaceMeasure::default();
        let r = add(&mut tx, "r", ElementKind::Rectangle);
        let l = add(
            &mut tx,
            "l",
            ElementKind::Line(LinearData::new(vec![kurbo::Point::ZERO, kurbo::Point::new(100.0, 60.0)])),
        );
        let t = add(&mut tx, "t", ElementKind::Text(drafftink_scene::TextData::new("hi")));
        tx.update(&r, |e| e.locked = true).unwrap();
        assert_eq!(convert(&mut tx, &m, &[], None, false), Err(ConvertError::NoElements));
        assert_eq!(convert(&mut tx, &m, &ids(&[&r]), None, false), Err(ConvertError::Locked(vec![r.clone()])));
        tx.update(&r, |e| e.locked = false).unwrap();
        assert_eq!(convert(&mut tx, &m, &ids(&[&r, &l]), None, false), Err(ConvertError::Mixed));
        assert!(matches!(
            convert(&mut tx, &m, &ids(&[&t]), None, false),
            Err(ConvertError::NotConvertible { kind: "text", .. })
        ));
        assert!(matches!(
            convert(&mut tx, &m, &ids(&[&r]), Some(Target::Line), false),
            Err(ConvertError::WrongFamily { from: "rectangle", to: Target::Line, .. })
        ));
        assert_eq!(
            convert(&mut tx, &m, &ids(&[&r]), Some(Target::Rectangle), false),
            Err(ConvertError::AlreadyThere { to: Target::Rectangle })
        );
        assert_eq!(tx.get(&r).unwrap().tag(), Kind::Rectangle);
    }

    #[test]
    fn a_conversion_reports_what_changed_and_what_was_left() {
        let mut store = MemoryStore::new();
        let mut tx = store.begin();
        let m = MonospaceMeasure::default();
        let a = add(&mut tx, "a", ElementKind::Rectangle);
        let b = add(&mut tx, "b", ElementKind::Ellipse);
        let done = convert(&mut tx, &m, &ids(&[&a, &b]), Some(Target::Ellipse), false).unwrap();
        assert_eq!((done.to, done.changed, done.unchanged), (Target::Ellipse, vec![a.clone()], vec![b]));
        assert_eq!(tx.get(&a).unwrap().tag(), Kind::Ellipse);
    }

    #[test]
    fn the_words_read_in_plain_language() {
        assert_eq!(Target::Arrow(PathType::Elbow).words(), "elbow arrow");
        assert_eq!(Target::Line.family(), Family::Linear);
        assert_eq!(Target::Diamond.family(), Family::Shape);
    }
}
