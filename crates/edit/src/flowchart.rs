//! Flowchart helpers: "add a connected node in a direction" (`diagram.extend`, Mod+Arrow) and arrow-key navigation
//! along connections (`selection.navigate`).
//!
//! The idea and the navigation tiers are adapted from Excalidraw (MIT), `element/src/flowchart.ts`; see
//! `ATTRIBUTION.md`.
//!
//! The new node copies the kind, size and style of the node it grows from. It goes `gap` beyond the near edge in
//! the chosen direction, centred on the parent. When that spot is taken, the other free spots are tried in a fixed
//! order: sliding sideways (centre, then one step to the positive side, then one to the negative side, then two,
//! and so on), then one more row out. [`plan_nodes`] lists these spots so a UI can preview them and cycle through
//! them with [`next_slot`]; [`create_connected_nodes`] creates the nodes and their bound arrows.

use drafftink_geom::OrientedBox;
use kurbo::{Point, Rect};

use drafftink_scene::{Element, ElementId, Head, IdSource, IndexError, Kind, PathType, SceneRead, SceneTx, StoreError};

use crate::binding::update_bound_arrows;
use crate::ids::unused_element_id;
use crate::layout::{Direction, Heads, NodeBox, RouteParams, arrow_element, route_edge};
use crate::text_bind::{TextBindError, TextMeasure, create_label};

/// The default distance between a node and the node grown from it.
pub const DEFAULT_GAP: f64 = 80.0;
/// How many sideways steps are tried on each side, and how many rows out.
const SIDE_STEPS: i32 = 8;
const ROWS: usize = 8;

/// Why a flowchart step refused.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum FlowchartError {
    /// The element does not exist.
    #[error("element {0} does not exist. Look up ids with element.query or doc.outline.")]
    Missing(ElementId),
    /// Only rectangles, diamonds and ellipses can grow nodes.
    #[error("{id} is {}. Connected nodes can be added to a rectangle, diamond or ellipse.", drafftink_scene::words::Article::new(.kind))]
    WrongKind {
        /// The element.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
    },
    /// The element is locked.
    #[error("{0} is locked. Unlock it first.")]
    Locked(ElementId),
    /// The gap is not a number of 0 or more.
    #[error("The gap must be a finite number of 0 or more.")]
    BadGap,
    /// A label could not be made.
    #[error(transparent)]
    Text(#[from] TextBindError),
    /// The scene refused a write.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// No z-order key could be made.
    #[error(transparent)]
    Index(#[from] IndexError),
    /// The geometry came out non-finite.
    #[error("The new node would have coordinates that are not finite numbers.")]
    NotFinite,
    /// Re-solving the arrows failed.
    #[error("re-solving the bound arrows failed: {0}")]
    Binding(String),
}

impl FlowchartError {
    /// The refusal code of the command catalog.
    pub fn code(&self) -> &'static str {
        match self {
            FlowchartError::Missing(_) => "not_found",
            FlowchartError::WrongKind { .. } => "wrong_kind",
            FlowchartError::Locked(_) => "locked",
            FlowchartError::BadGap | FlowchartError::NotFinite => "bad_args",
            _ => "internal",
        }
    }
}

/// Options for new nodes.
#[derive(Debug, Clone)]
pub struct ExtendOpts {
    /// Distance between the parent's edge and the new node.
    pub gap: f64,
    /// Which free spot to start from (0 is the nearest to straight ahead). Wraps around.
    pub first_slot: usize,
    /// Text of the new node.
    pub label: Option<String>,
    /// Text on the new arrow.
    pub arrow_label: Option<String>,
    /// Shape of the arrow.
    pub path: PathType,
    /// Heads of the arrow.
    pub heads: Heads,
}

impl Default for ExtendOpts {
    fn default() -> Self {
        Self {
            gap: DEFAULT_GAP,
            first_slot: 0,
            label: None,
            arrow_label: None,
            path: PathType::Sharp,
            heads: Heads { start: Head::None, end: Head::Arrow },
        }
    }
}

/// What was created for one new node.
#[derive(Debug, Clone, PartialEq)]
pub struct Extended {
    /// The new node.
    pub node: ElementId,
    /// The arrow from the parent to the node.
    pub arrow: ElementId,
    /// The node's label, when it has one.
    pub node_label: Option<ElementId>,
    /// The arrow's label, when it has one.
    pub arrow_label: Option<ElementId>,
    /// The node's box.
    pub rect: Rect,
}

fn is_obstacle(e: &Element) -> bool {
    !e.deleted
        && e.container_id().is_none()
        && matches!(e.tag(), Kind::Rectangle | Kind::Diamond | Kind::Ellipse | Kind::Text | Kind::Image | Kind::Math)
}

fn live<'a, R: SceneRead>(r: &'a R, id: &ElementId) -> Result<&'a Element, FlowchartError> {
    r.get(id).filter(|e| !e.deleted).ok_or_else(|| FlowchartError::Missing(id.clone()))
}

fn check_parent<'a, R: SceneRead>(r: &'a R, id: &ElementId) -> Result<&'a Element, FlowchartError> {
    let e = live(r, id)?;
    if !matches!(e.tag(), Kind::Rectangle | Kind::Diamond | Kind::Ellipse) {
        return Err(FlowchartError::WrongKind { id: id.clone(), kind: e.tag().name() });
    }
    if e.locked {
        return Err(FlowchartError::Locked(id.clone()));
    }
    Ok(e)
}

fn aabb(e: &Element) -> Rect {
    let t = &e.transform;
    OrientedBox::from_xywh(t.x, t.y, t.w, t.h, t.angle).aabb()
}

/// The free spots for new nodes of the parent's size next to `from`, nearest to straight ahead first. At most
/// `count` are returned (fewer when the space around is full).
pub fn plan_nodes<R: SceneRead>(
    r: &R,
    from: &ElementId,
    dir: Direction,
    gap: f64,
    count: usize,
) -> Result<Vec<Rect>, FlowchartError> {
    if !gap.is_finite() || gap < 0.0 {
        return Err(FlowchartError::BadGap);
    }
    let parent = check_parent(r, from)?;
    let pr = aabb(parent);
    let size = parent.rect().size();
    let clearance = gap / 4.0;
    let obstacles: Vec<Rect> = r
        .iter()
        .filter(|e| e.id != parent.id && is_obstacle(e))
        .map(aabb)
        .map(|b| b.inflate(clearance, clearance))
        .collect();
    let step_cross = |vertical: bool| if vertical { size.width } else { size.height } + gap.max(clearance);
    let vertical = matches!(dir, Direction::Up | Direction::Down);
    let step_main = if vertical { size.height } else { size.width } + gap;
    let centre = pr.center();
    let mut out: Vec<Rect> = Vec::new();
    'rows: for row in 0..ROWS {
        let main_off = gap + row as f64 * step_main;
        for k in 0..=(2 * SIDE_STEPS) {
            // 0, +1, -1, +2, -2, ...
            let side = if k == 0 {
                0
            } else if k % 2 == 1 {
                (k + 1) / 2
            } else {
                -(k / 2)
            };
            let cross_off = f64::from(side) * step_cross(vertical);
            let (cx, cy) = match dir {
                Direction::Down => (centre.x + cross_off, pr.y1 + main_off + size.height / 2.0),
                Direction::Up => (centre.x + cross_off, pr.y0 - main_off - size.height / 2.0),
                Direction::Right => (pr.x1 + main_off + size.width / 2.0, centre.y + cross_off),
                Direction::Left => (pr.x0 - main_off - size.width / 2.0, centre.y + cross_off),
            };
            let rect = Rect::from_center_size(Point::new(cx, cy), size);
            if obstacles.iter().any(|o| o.x0 < rect.x1 && rect.x0 < o.x1 && o.y0 < rect.y1 && rect.y0 < o.y1) {
                continue;
            }
            out.push(rect);
            if out.len() >= count {
                break 'rows;
            }
        }
    }
    Ok(out)
}

/// The slot after `current`, wrapping at `available`. For cycling through candidate spots with a key.
pub fn next_slot(current: usize, available: usize) -> usize {
    if available == 0 { 0 } else { (current + 1) % available }
}

/// Creates one node per planned spot, starting at `opts.first_slot`, each with an arrow from `from`. The nodes copy
/// the parent's kind, size and style. Returns what was created, in order. Creates nothing when no spot is free.
pub fn create_connected_nodes<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &mut dyn IdSource,
    from: &ElementId,
    dir: Direction,
    opts: &ExtendOpts,
    count: usize,
) -> Result<Vec<Extended>, FlowchartError> {
    let want = count.max(1);
    // Plan enough spots to start at `first_slot` and wrap around.
    let all = plan_nodes(tx, from, dir, opts.gap, opts.first_slot.saturating_add(want).min(64))?;
    if all.is_empty() {
        return Ok(Vec::new());
    }
    let parent = check_parent(tx, from)?.clone();
    let mut out = Vec::new();
    for i in 0..want.min(all.len()) {
        let rect = all[(opts.first_slot + i) % all.len()];
        if !(rect.x0.is_finite() && rect.y0.is_finite()) {
            return Err(FlowchartError::NotFinite);
        }
        out.push(create_one(tx, m, ids, &parent, dir, opts, rect)?);
    }
    let mut touched = vec![from.clone()];
    touched.extend(out.iter().map(|e| e.node.clone()));
    update_bound_arrows(tx, &touched).map_err(|e| FlowchartError::Binding(e.to_string()))?;
    Ok(out)
}

fn create_one<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &mut dyn IdSource,
    parent: &Element,
    dir: Direction,
    opts: &ExtendOpts,
    rect: Rect,
) -> Result<Extended, FlowchartError> {
    // The frame is kept when the new node lies inside it.
    let frame = parent
        .frame_id
        .as_ref()
        .and_then(|f| tx.get(f))
        .filter(|f| !f.deleted && f.rect().contains_rect(rect))
        .map(|f| f.id.clone());
    let index_for = |tx: &T| -> Result<_, FlowchartError> {
        match frame.as_ref().and_then(|f| tx.get(f)) {
            Some(f) => {
                let (below, _) = tx.z_neighbors(&f.id);
                Ok(drafftink_scene::ZIndex::between(below.as_ref(), Some(&f.index))?)
            }
            None => Ok(tx.next_top_index()?),
        }
    };

    let node_id = unused_element_id(tx, ids);
    let mut node = Element::new(node_id.clone(), parent.kind.clone(), index_for(tx)?)
        .with_bounds(rect.x0, rect.y0, rect.width(), rect.height())
        .with_style(parent.style.clone());
    node.style.seed = ids.seed();
    node.frame_id = frame.clone();
    let node_box = NodeBox::of(&node).ok_or(FlowchartError::NotFinite)?;
    tx.insert(node)?;

    let parent_box = NodeBox::of(parent).ok_or(FlowchartError::NotFinite)?;
    let params = RouteParams { path: opts.path, waypoints: &[], flow: Some(dir), lane: (0, 1), obstacles: &[] };
    let route = route_edge(&parent_box, &node_box, &params);
    let arrow_id = unused_element_id(tx, ids);
    let mut arrow = arrow_element(arrow_id.clone(), index_for(tx)?, &route, opts.path, opts.heads)
        .ok_or(FlowchartError::NotFinite)?;
    arrow.style.stroke_color = parent.style.stroke_color;
    arrow.style.stroke_width = parent.style.stroke_width;
    arrow.style.opacity = parent.style.opacity;
    arrow.style.sloppiness = parent.style.sloppiness;
    arrow.style.seed = ids.seed();
    arrow.frame_id = frame;
    tx.insert(arrow)?;

    let node_label = match opts.label.as_deref().filter(|l| !l.trim().is_empty()) {
        Some(text) => Some(create_label(tx, m, ids, &node_id, text, None)?.label),
        None => None,
    };
    let arrow_label = match opts.arrow_label.as_deref().filter(|l| !l.trim().is_empty()) {
        Some(text) => Some(create_label(tx, m, ids, &arrow_id, text, None)?.label),
        None => None,
    };
    let rect = tx.get(&node_id).map_or(rect, |e| e.rect());
    Ok(Extended { node: node_id, arrow: arrow_id, node_label, arrow_label, rect })
}

/// Convenience for one node: `diagram.extend`.
pub fn create_connected_node<T: SceneTx, M: TextMeasure + ?Sized>(
    tx: &mut T,
    m: &M,
    ids: &mut dyn IdSource,
    from: &ElementId,
    dir: Direction,
    opts: &ExtendOpts,
) -> Result<Option<Extended>, FlowchartError> {
    Ok(create_connected_nodes(tx, m, ids, from, dir, opts, 1)?.into_iter().next())
}

// ---------------------------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------------------------

/// The node connected to `from` by an arrow that lies in direction `dir` (`selection.navigate`), or `None`.
///
/// Candidates are the shapes joined to `from` by an arrow in either direction whose centre is past the centre of
/// `from` in `dir`. A candidate straight ahead (the boxes overlap across the direction) wins over one in the 45
/// degree cone, which wins over the rest. Within a tier the nearest wins; near ties (within 10%) prefer a target
/// of an arrow from `from` over a source, then the smaller sideways offset, then the lower id.
pub fn navigate<R: SceneRead>(r: &R, from: &ElementId, dir: Direction) -> Option<ElementId> {
    let me = r.get(from).filter(|e| !e.deleted)?;
    let mr = aabb(me);
    let mc = mr.center();
    let mut cands: Vec<(ElementId, bool)> = Vec::new(); // (id, is successor)
    for e in r.iter().filter(|e| e.tag() == Kind::Arrow) {
        let [s, t] = e.binding_targets();
        match (s, t) {
            (Some(s), Some(t)) if s == from && t != from => cands.push((t.clone(), true)),
            (Some(s), Some(t)) if t == from && s != from => cands.push((s.clone(), false)),
            _ => {}
        }
    }
    struct Scored {
        id: ElementId,
        tier: u8,
        dist: f64,
        link: u8,
        cross: f64,
    }
    let mut scored: Vec<Scored> = Vec::new();
    for (id, successor) in cands {
        let Some(e) = r.get(&id).filter(|e| !e.deleted && matches!(e.tag(), k if k.is_bindable())) else { continue };
        let rect = aabb(e);
        let c = rect.center();
        let (main, cross, gap_main, overlap_cross) = match dir {
            Direction::Right => (c.x - mc.x, c.y - mc.y, rect.x0 - mr.x1, mr.y0 < rect.y1 && rect.y0 < mr.y1),
            Direction::Left => (mc.x - c.x, c.y - mc.y, mr.x0 - rect.x1, mr.y0 < rect.y1 && rect.y0 < mr.y1),
            Direction::Down => (c.y - mc.y, c.x - mc.x, rect.y0 - mr.y1, mr.x0 < rect.x1 && rect.x0 < mr.x1),
            Direction::Up => (mc.y - c.y, c.x - mc.x, mr.y0 - rect.y1, mr.x0 < rect.x1 && rect.x0 < mr.x1),
        };
        if main <= 0.0 {
            continue;
        }
        let tier = if overlap_cross {
            0
        } else if main >= cross.abs() {
            1
        } else {
            2
        };
        let dist = if tier == 0 { gap_main.max(0.0) } else { main.hypot(cross) };
        scored.push(Scored { id, tier, dist, link: u8::from(!successor), cross: cross.abs() });
    }
    scored.sort_by(|a, b| a.tier.cmp(&b.tier).then(a.dist.total_cmp(&b.dist)));
    let best = scored.first()?;
    let tol = (best.dist * 0.1).max(1.0);
    scored
        .iter()
        .filter(|s| s.tier == best.tier && s.dist - best.dist <= tol)
        .min_by(|a, b| {
            a.link.cmp(&b.link).then(a.cross.total_cmp(&b.cross)).then(a.dist.total_cmp(&b.dist)).then(a.id.cmp(&b.id))
        })
        .map(|s| s.id.clone())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::text_bind::MonospaceMeasure;
    use drafftink_scene::{ElementKind, MemoryStore, SceneStore, SeqIds, validate_scene};

    fn shape(store: &mut MemoryStore, id: &str, x: f64, y: f64) -> ElementId {
        shape_kind(store, id, ElementKind::Rectangle, x, y)
    }

    fn shape_kind(store: &mut MemoryStore, id: &str, kind: ElementKind, x: f64, y: f64) -> ElementId {
        let mut tx = store.begin();
        let z = tx.next_top_index().unwrap();
        let id = ElementId::new(id).unwrap();
        tx.insert(Element::new(id.clone(), kind, z).with_bounds(x, y, 100.0, 60.0)).unwrap();
        tx.commit().unwrap();
        id
    }

    fn extend(store: &mut MemoryStore, from: &ElementId, dir: Direction, opts: &ExtendOpts) -> Vec<Extended> {
        let mut ids = SeqIds::new();
        // Keep generated ids clear of the test ids.
        for _ in 0..100 {
            ids.element_id();
        }
        let mut tx = store.begin();
        let r = create_connected_nodes(&mut tx, &MonospaceMeasure::default(), &mut ids, from, dir, opts, 1).unwrap();
        tx.commit().unwrap();
        r
    }

    #[test]
    fn grows_a_node_straight_ahead_with_a_bound_arrow() {
        let mut s = MemoryStore::new();
        let a = shape(&mut s, "a", 0.0, 0.0);
        let made =
            extend(&mut s, &a, Direction::Right, &ExtendOpts { label: Some("next".into()), ..ExtendOpts::default() });
        assert_eq!(made.len(), 1);
        let n = s.get(&made[0].node).unwrap();
        assert_eq!(n.tag(), Kind::Rectangle);
        assert_eq!((n.transform.x, n.transform.y, n.transform.w, n.transform.h), (180.0, 0.0, 100.0, 60.0));
        let arrow = s.get(&made[0].arrow).unwrap();
        let [st, en] = arrow.binding_targets();
        assert_eq!((st, en), (Some(&a), Some(&made[0].node)));
        assert_eq!(arrow.linear_data().unwrap().end_head, Head::Arrow);
        assert!(made[0].node_label.is_some());
        assert!(!validate_scene(&s).has_errors());
        // The arrow runs left to right between the two boxes.
        let pts = &arrow.linear_data().unwrap().points;
        assert!(arrow.transform.x >= 100.0 && arrow.transform.x + arrow.transform.w <= 180.0, "{pts:?}");
    }

    #[test]
    fn copies_kind_size_and_style() {
        let mut s = MemoryStore::new();
        let a = shape_kind(&mut s, "a", ElementKind::Diamond, 0.0, 0.0);
        let mut tx = s.begin();
        tx.update(&a, |e| {
            e.style.stroke_width = 5.0;
            e.name = Some("keep".into());
        })
        .unwrap();
        tx.commit().unwrap();
        let made = extend(&mut s, &a, Direction::Down, &ExtendOpts::default());
        let n = s.get(&made[0].node).unwrap();
        assert_eq!(n.tag(), Kind::Diamond);
        assert_eq!(n.style.stroke_width, 5.0);
        assert_eq!(n.name, None);
        assert_eq!((n.transform.x, n.transform.y), (0.0, 140.0));
    }

    #[test]
    fn all_four_directions() {
        for (dir, expect) in [
            (Direction::Up, (0.0, -140.0)),
            (Direction::Down, (0.0, 140.0)),
            (Direction::Left, (-180.0, 0.0)),
            (Direction::Right, (180.0, 0.0)),
        ] {
            let mut s = MemoryStore::new();
            let a = shape(&mut s, "a", 0.0, 0.0);
            let made = extend(&mut s, &a, dir, &ExtendOpts::default());
            let n = s.get(&made[0].node).unwrap();
            assert_eq!((n.transform.x, n.transform.y), expect, "{dir:?}");
        }
    }

    #[test]
    fn a_taken_spot_slides_sideways_and_siblings_do_not_overlap() {
        let mut s = MemoryStore::new();
        let a = shape(&mut s, "a", 0.0, 0.0);
        let mut seen: Vec<Rect> = Vec::new();
        for _ in 0..6 {
            let made = extend(&mut s, &a, Direction::Down, &ExtendOpts::default());
            let r = s.get(&made[0].node).unwrap().rect();
            assert!(
                seen.iter().all(|o| !(o.x0 < r.x1 && r.x0 < o.x1 && o.y0 < r.y1 && r.y0 < o.y1)),
                "{r:?} overlaps {seen:?}"
            );
            seen.push(r);
        }
        // Order: straight ahead, then positive side, then negative side.
        assert_eq!(seen[0].x0, 0.0);
        assert!(seen[1].x0 > 0.0);
        assert!(seen[2].x0 < 0.0);
        assert!(seen.iter().all(|r| r.y0 == 140.0));
    }

    #[test]
    fn obstacles_far_away_in_the_row_push_to_the_next_row() {
        let mut s = MemoryStore::new();
        let a = shape(&mut s, "a", 0.0, 0.0);
        // A wide wall below blocks the whole first row.
        let mut tx = s.begin();
        let z = tx.next_top_index().unwrap();
        tx.insert(
            Element::new(ElementId::new("wall").unwrap(), ElementKind::Rectangle, z)
                .with_bounds(-5000.0, 100.0, 10000.0, 200.0),
        )
        .unwrap();
        tx.commit().unwrap();
        let made = extend(&mut s, &a, Direction::Down, &ExtendOpts::default());
        let r = s.get(&made[0].node).unwrap().rect();
        assert!(r.y0 >= 300.0, "{r:?}");
    }

    #[test]
    fn plan_lists_candidates_and_slots_cycle() {
        let mut s = MemoryStore::new();
        let a = shape(&mut s, "a", 0.0, 0.0);
        let plan = plan_nodes(&s, &a, Direction::Right, DEFAULT_GAP, 3).unwrap();
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].y0, 0.0);
        assert!(plan[1].y0 > 0.0 && plan[2].y0 < 0.0);
        assert_eq!(next_slot(2, 3), 0);
        assert_eq!(next_slot(0, 0), 0);
        let made = extend(&mut s, &a, Direction::Right, &ExtendOpts { first_slot: 1, ..ExtendOpts::default() });
        assert_eq!(s.get(&made[0].node).unwrap().rect(), plan[1]);
    }

    #[test]
    fn refusals_are_typed() {
        let mut s = MemoryStore::new();
        let a = shape(&mut s, "a", 0.0, 0.0);
        let t = shape_kind(&mut s, "t", ElementKind::Text(drafftink_scene::TextData::new("x")), 300.0, 0.0);
        let mut ids = SeqIds::new();
        let mut tx = s.begin();
        let m = MonospaceMeasure::default();
        let o = ExtendOpts::default();
        let err = create_connected_nodes(&mut tx, &m, &mut ids, &t, Direction::Up, &o, 1).unwrap_err();
        assert_eq!(err.code(), "wrong_kind");
        let missing = ElementId::new("zz").unwrap();
        assert_eq!(
            create_connected_nodes(&mut tx, &m, &mut ids, &missing, Direction::Up, &o, 1).unwrap_err().code(),
            "not_found"
        );
        tx.update(&a, |e| e.locked = true).unwrap();
        assert_eq!(
            create_connected_nodes(&mut tx, &m, &mut ids, &a, Direction::Up, &o, 1).unwrap_err().code(),
            "locked"
        );
        let bad = ExtendOpts { gap: f64::NAN, ..ExtendOpts::default() };
        tx.update(&a, |e| e.locked = false).unwrap();
        assert_eq!(
            create_connected_nodes(&mut tx, &m, &mut ids, &a, Direction::Up, &bad, 1).unwrap_err(),
            FlowchartError::BadGap
        );
    }

    #[test]
    fn child_stays_in_the_frame_below_it() {
        let mut s = MemoryStore::new();
        let mut tx = s.begin();
        let z = tx.next_top_index().unwrap();
        let fid = ElementId::new("f").unwrap();
        let a = ElementId::new("a").unwrap();
        tx.insert(Element::new(a.clone(), ElementKind::Rectangle, z).with_bounds(20.0, 20.0, 100.0, 60.0)).unwrap();
        tx.update(&a, |e| e.frame_id = Some(fid.clone())).unwrap();
        let z = tx.next_top_index().unwrap();
        tx.insert(
            Element::new(fid.clone(), ElementKind::Frame(Default::default()), z).with_bounds(0.0, 0.0, 600.0, 600.0),
        )
        .unwrap();
        tx.commit().unwrap();
        let made = extend(&mut s, &a, Direction::Down, &ExtendOpts::default());
        let n = s.get(&made[0].node).unwrap();
        assert_eq!(n.frame_id.as_ref(), Some(&fid));
        let report = validate_scene(&s);
        assert!(!report.has_errors(), "{report:?}");
        // A node that would fall outside the frame does not join it.
        let made =
            extend(&mut s, &made[0].node.clone(), Direction::Down, &ExtendOpts { gap: 900.0, ..ExtendOpts::default() });
        assert_eq!(s.get(&made[0].node).unwrap().frame_id, None);
    }

    #[test]
    fn navigate_follows_arrows_by_direction() {
        let mut s = MemoryStore::new();
        let a = shape(&mut s, "a", 0.0, 0.0);
        let r1 = extend(&mut s, &a, Direction::Right, &ExtendOpts::default());
        let r2 = extend(&mut s, &a, Direction::Down, &ExtendOpts::default());
        assert_eq!(navigate(&s, &a, Direction::Right), Some(r1[0].node.clone()));
        assert_eq!(navigate(&s, &a, Direction::Down), Some(r2[0].node.clone()));
        assert_eq!(navigate(&s, &a, Direction::Up), None);
        assert_eq!(navigate(&s, &a, Direction::Left), None);
        // Back along the arrow.
        assert_eq!(navigate(&s, &r1[0].node, Direction::Left), Some(a.clone()));
        assert_eq!(navigate(&s, &ElementId::new("nope").unwrap(), Direction::Left), None);
    }
}
