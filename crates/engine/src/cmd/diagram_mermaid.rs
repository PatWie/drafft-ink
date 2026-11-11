//! Maps a parsed Mermaid diagram (`drafftink_io::mermaid`) into the diagram spec of `drafftink_edit::layout`.
//!
//! This is the one place that knows both formats: `io` and `edit` are both L4 and cannot depend on each other,
//! so the engine does the mapping. Flowcharts become a [`DiagramSpec`] that `diagram_create` lays out in the
//! flow direction. Sequence diagrams become participant and note nodes at fixed positions (the `none` layout)
//! plus lifelines and message arrows that [`Sequence`] describes in coordinates relative to the diagram.

use std::collections::{BTreeMap, BTreeSet};

use drafftink_edit::layout::{
    Algorithm, Arrowheads, ClusterSpec, DiagramSpec, Direction, EdgeSpec, GroupSpec, LayoutOpts, NodeShape as SpecShape,
    NodeSpec, StyleSpec,
};
use drafftink_edit::text_bind::{TextMeasure, create_label, measure_text};
use drafftink_edit::unused_element_id;
use drafftink_geom::kurbo::{Point, Rect, Vec2};
use drafftink_io::IoError;
use drafftink_io::mermaid::{self as mm, Diagram};
use drafftink_scene::{Element, ElementId, ElementKind, FillPattern, Head, IdSource, LinearKind, PathType, SceneTx, StrokeStyle, TextStyle};

use crate::refusal::Refusal;
use crate::session::Session;

/// A message arrow of a sequence diagram, in diagram coordinates.
#[derive(Debug, Clone)]
pub(super) struct Message {
    /// Start `(x, y)`.
    pub from: (f64, f64),
    /// End `(x, y)`. Equal `x` for a message to self (drawn as a loop).
    pub to: (f64, f64),
    /// Text on the arrow.
    pub text: String,
    /// Dashed line.
    pub dashed: bool,
    /// Marker at the end.
    pub head: Head,
    /// Marker at the start.
    pub start_head: Head,
}

/// What a sequence diagram needs beyond nodes: lifelines and messages.
#[derive(Debug, Clone, Default)]
pub(super) struct Sequence {
    /// `(x, y_start, y_end)` of each dashed lifeline.
    pub lifelines: Vec<(f64, f64, f64)>,
    /// The message arrows.
    pub messages: Vec<Message>,
    /// Bounds of the whole diagram.
    pub bounds: Rect,
}

/// The result of the mapping.
#[derive(Debug, Clone)]
pub(super) struct Built {
    /// Nodes, edges and layout.
    pub spec: DiagramSpec,
    /// Present for sequence diagrams.
    pub seq: Option<Sequence>,
    /// Group the whole result (the `group` argument).
    pub group_all: bool,
}

const UNSUPPORTED: &[&str] = &[
    "classDiagram",
    "stateDiagram",
    "stateDiagram-v2",
    "erDiagram",
    "gantt",
    "pie",
    "journey",
    "mindmap",
    "gitGraph",
    "timeline",
    "quadrantChart",
    "requirementDiagram",
    "sankey-beta",
    "xychart-beta",
    "block-beta",
    "architecture-beta",
    "packet-beta",
    "kanban",
    "C4Context",
    "C4Container",
    "C4Component",
    "C4Dynamic",
    "C4Deployment",
];

/// The first meaningful line of the text and its 1-based number: skips blank lines, `%%` comments, a code fence
/// and a front-matter block.
fn header_line(text: &str) -> Option<(usize, &str)> {
    let mut in_front = false;
    let mut seen_front = false;
    for (i, raw) in text.lines().enumerate() {
        let l = raw.trim();
        if l.is_empty() || l.starts_with("%%") || l.starts_with("```") || l.starts_with("~~~") {
            continue;
        }
        if l == "---" && (in_front || !seen_front) {
            in_front = !in_front;
            seen_front = true;
            continue;
        }
        if in_front {
            continue;
        }
        return Some((i + 1, l));
    }
    None
}

fn refuse_io(cmd: &str, text: &str, e: IoError) -> Refusal {
    let hint = "Fix that line and try again.";
    match &e {
        IoError::WrongFormat { .. } | IoError::Invalid { .. } | IoError::Parse { .. } => {
            let (n, first) = header_line(text).unwrap_or((1, ""));
            let word = first.split_whitespace().next().unwrap_or("");
            if matches!(e, IoError::WrongFormat { .. }) && UNSUPPORTED.contains(&word) {
                return Refusal::custom(
                    "mermaid_unsupported",
                    cmd,
                    format!("Mermaid diagram type `{word}` is not supported yet. Supported: flowchart/graph and sequenceDiagram."),
                );
            }
            Refusal::parse_failed(cmd, "The Mermaid text", &format!("{e} (line {n})"), hint)
        }
        IoError::Limit { .. } => Refusal::custom(
            "limit_exceeded",
            cmd,
            format!("The Mermaid text is too large: {e}. Split it into several smaller diagrams."),
        ),
        other => Refusal::parse_failed(cmd, "The Mermaid text", &other.to_string(), hint),
    }
}

/// Parses and maps. Returns the spec and the notes about what Mermaid features were approximated or dropped.
pub(super) fn to_spec(cmd: &str, text: &str, group: bool, s: &Session) -> Result<(Built, Vec<String>), Refusal> {
    let diagram = mm::parse(text).map_err(|e| refuse_io(cmd, text, e))?;
    match diagram {
        Diagram::Flowchart(g) => Ok(flowchart(&g, group)),
        Diagram::Sequence(q) => Ok(sequence(&q, group, s.services.edit())),
    }
}

fn head(h: mm::Head) -> Head {
    match h {
        mm::Head::None => Head::None,
        mm::Head::Arrow => Head::Arrow,
        mm::Head::Cross => Head::Bar,
        mm::Head::Circle => Head::CircleOutline,
    }
}

fn direction(d: mm::Direction) -> Direction {
    match d {
        mm::Direction::TopToBottom => Direction::Down,
        mm::Direction::BottomToTop => Direction::Up,
        mm::Direction::LeftToRight => Direction::Right,
        mm::Direction::RightToLeft => Direction::Left,
    }
}

/// How a Mermaid node outline is drawn. Arrows bind to rectangles, ellipses and diamonds only, so every Mermaid
/// outline maps to one of those three.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DrawnShape {
    shape: SpecShape,
    /// Fixed `[width, height]`, for the round shapes.
    size: Option<[f64; 2]>,
    corner_radius: Option<f64>,
    /// The Mermaid name of the shape when the drawing only resembles it, for the warning.
    approximates: Option<&'static str>,
}

impl DrawnShape {
    const fn exact(shape: SpecShape) -> Self {
        Self { shape, size: None, corner_radius: None, approximates: None }
    }

    const fn rounded(radius: f64) -> Self {
        Self { shape: SpecShape::Rectangle, size: None, corner_radius: Some(radius), approximates: None }
    }

    /// The same drawing, reported as an approximation of `name`.
    const fn approximating(self, name: &'static str) -> Self {
        Self { approximates: Some(name), ..self }
    }
}

/// Side of the round nodes. A double circle gets a larger one so that it stays apart from a circle.
const CIRCLE: f64 = 80.0;
const DOUBLE_CIRCLE: f64 = 96.0;

/// The closest drawable shape for a Mermaid node shape. Hexagon, cylinder, parallelograms, trapezoids, the flag
/// and the subroutine would need an outline that the scene has no element for (a polygon cannot hold a label or
/// take a binding), so they are rectangles whose corner rounding tells them apart from a plain box.
fn drawn_shape(shape: mm::NodeShape) -> DrawnShape {
    use mm::NodeShape as M;
    match shape {
        M::Rectangle => DrawnShape::exact(SpecShape::Rectangle),
        M::Rounded => DrawnShape::rounded(16.0),
        M::Stadium => DrawnShape::rounded(24.0),
        M::Circle => DrawnShape { size: Some([CIRCLE; 2]), ..DrawnShape::exact(SpecShape::Ellipse) },
        M::DoubleCircle => {
            DrawnShape { size: Some([DOUBLE_CIRCLE; 2]), ..DrawnShape::exact(SpecShape::Ellipse) }.approximating("double circle")
        }
        M::Diamond => DrawnShape::exact(SpecShape::Diamond),
        M::Subroutine => DrawnShape::exact(SpecShape::Rectangle).approximating("subroutine"),
        M::Hexagon => DrawnShape::rounded(20.0).approximating("hexagon"),
        M::Cylinder => DrawnShape::rounded(10.0).approximating("cylinder"),
        M::Parallelogram | M::ParallelogramAlt => DrawnShape::rounded(6.0).approximating("parallelogram"),
        M::Trapezoid | M::TrapezoidAlt => DrawnShape::rounded(4.0).approximating("trapezoid"),
        M::Asymmetric => DrawnShape::rounded(2.0).approximating("flag"),
    }
}

fn flowchart(g: &mm::MermaidGraph, group: bool) -> (Built, Vec<String>) {
    let mut warnings: Vec<String> = g.warnings.iter().take(10).cloned().collect();
    // Symbolic ids: the Mermaid id when it fits, else a short stand-in.
    let sym: Vec<String> = g
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let len = n.id.chars().count();
            if (1..=64).contains(&len) && !n.id.chars().any(char::is_control) { n.id.clone() } else { format!("n{i}") }
        })
        .collect();
    let by_mermaid: BTreeMap<&str, &str> = g.nodes.iter().zip(&sym).map(|(n, s)| (n.id.as_str(), s.as_str())).collect();
    let mut approximated: BTreeSet<&'static str> = BTreeSet::new();
    let nodes: Vec<NodeSpec> = g
        .nodes
        .iter()
        .zip(&sym)
        .map(|(n, id)| {
            let mut style = StyleSpec::default();
            let drawn = drawn_shape(n.shape);
            if let Some(name) = drawn.approximates {
                approximated.insert(name);
            }
            let (shape, size) = (drawn.shape, drawn.size);
            style.corner_radius = drawn.corner_radius;
            if let Some(st) = &n.style {
                if let Some(c) = st.fill {
                    style.fill_color = Some(c);
                    style.fill_pattern = Some(FillPattern::Solid);
                }
                style.stroke_color = st.stroke.or(style.stroke_color);
                style.stroke_width = st.stroke_width.filter(|w| *w <= 100.0).or(style.stroke_width);
            }
            let styled = style != StyleSpec::default();
            NodeSpec {
                id: id.clone(),
                label: Some(n.label.clone()),
                shape,
                style: styled.then_some(style),
                width: None,
                height: None,
                size,
                name: None,
                x: None,
                y: None,
            }
        })
        .collect();
    if !approximated.is_empty() {
        let names = approximated.into_iter().collect::<Vec<_>>().join(", ");
        warnings.push(format!(
            "Mermaid shapes that drafft.ink has no outline for were drawn as the closest shape (rectangle with rounded corners, or ellipse): {names}."
        ));
    }
    // Subgraphs become boxes with a title. The symbolic id of a box is the Mermaid id, or a stand-in that no node has.
    let mut taken: BTreeSet<&str> = sym.iter().map(String::as_str).collect();
    let cluster_syms: Vec<String> = g
        .subgraphs
        .iter()
        .enumerate()
        .map(|(i, sg)| {
            let len = sg.id.chars().count();
            let fits = (1..=64).contains(&len) && !sg.id.chars().any(char::is_control);
            let mut id = if fits { sg.id.clone() } else { format!("subgraph{i}") };
            while taken.contains(id.as_str()) {
                id.push('_');
            }
            id
        })
        .collect();
    taken.clear();
    let mut invisible = 0usize;
    let mut edges = Vec::new();
    for e in &g.edges {
        let end = |name: &str, subgraph: Option<usize>| match subgraph {
            Some(si) => cluster_syms.get(si).map(String::as_str),
            None => by_mermaid.get(name).copied(),
        };
        let (Some(f), Some(t)) = (end(&e.from, e.from_subgraph), end(&e.to, e.to_subgraph)) else { continue };
        if e.line == mm::Line::Invisible {
            invisible += 1;
            continue;
        }
        let mut style = StyleSpec::default();
        match e.line {
            mm::Line::Dotted => style.stroke_style = Some(StrokeStyle::Dotted),
            mm::Line::Thick => style.stroke_width = Some(4.0),
            _ => {}
        }
        edges.push(EdgeSpec {
            from: f.to_owned(),
            to: t.to_owned(),
            label: e.label.clone().filter(|l| !l.trim().is_empty()),
            arrow_type: Some(PathType::Sharp),
            arrowheads: Some(Arrowheads::None),
            start_head: Some(head(e.head_start)),
            end_head: Some(head(e.head_end)),
            style: (style != StyleSpec::default()).then_some(style),
            name: None,
        });
    }
    if invisible > 0 {
        warnings.push(format!("{invisible} invisible link(s) (~~~) were skipped; they only influence Mermaid's own layout."));
    }
    let clusters: Vec<ClusterSpec> = g
        .subgraphs
        .iter()
        .zip(&cluster_syms)
        .enumerate()
        .map(|(si, (sg, id))| ClusterSpec {
            id: id.clone(),
            title: sg.title.clone(),
            parent: sg.parent.and_then(|p| cluster_syms.get(p)).cloned(),
            members: g
                .nodes
                .iter()
                .zip(&sym)
                .filter(|(n, _)| n.subgraph == Some(si))
                .map(|(_, s)| s.clone())
                .collect(),
            direction: sg.direction.map(direction),
            group,
        })
        .collect();
    // One group around everything, unless a subgraph box already holds every node.
    let one_box_holds_all = group && g.subgraphs.iter().enumerate().any(|(si, _)| {
        g.nodes.iter().all(|n| std::iter::successors(n.subgraph, |&s| g.subgraphs.get(s).and_then(|s| s.parent)).any(|s| s == si))
    });
    let mut groups: Vec<GroupSpec> = Vec::new();
    if group && nodes.len() >= 2 && !one_box_holds_all {
        groups.push(GroupSpec { id: None, members: sym.clone() });
    }
    let layout = LayoutOpts { algorithm: Algorithm::Layered, direction: direction(g.direction), ..LayoutOpts::default() };
    let built = Built { spec: DiagramSpec { nodes, edges, groups, clusters, layout }, seq: None, group_all: false };
    (built, warnings)
}

const P_MIN_W: f64 = 120.0;
const P_H: f64 = 48.0;
const P_GAP: f64 = 60.0;
const ROW: f64 = 44.0;

fn sequence(q: &mm::SequenceDiagram, group: bool, m: &dyn TextMeasure) -> (Built, Vec<String>) {
    let mut warnings: Vec<String> = q.warnings.iter().take(10).cloned().collect();
    let font = TextStyle::default();
    let text_w = |t: &str| measure_text(m, t, &font, None);
    let mut x = 0.0;
    let mut centre: BTreeMap<&str, f64> = BTreeMap::new();
    let mut nodes: Vec<NodeSpec> = Vec::new();
    for (i, p) in q.participants.iter().enumerate() {
        let w = (text_w(&p.label).width + 40.0).max(P_MIN_W);
        let mut n = NodeSpec::new(format!("p{i}"), p.label.clone());
        n.shape = if p.is_actor { SpecShape::Ellipse } else { SpecShape::Rectangle };
        n.size = Some([w, P_H]);
        n.x = Some(x);
        n.y = Some(0.0);
        centre.insert(p.id.as_str(), x + w / 2.0);
        nodes.push(n);
        x += w + P_GAP;
    }
    let total_w = (x - P_GAP).max(P_MIN_W);
    let mut y = P_H + ROW;
    let mut messages = Vec::new();
    let mut notes = 0usize;
    for ev in &q.events {
        match ev {
            mm::Event::Message { from, to, text, dashed, head: h, bidirectional, .. } => {
                let (Some(&x0), Some(&x1)) = (centre.get(from.as_str()), centre.get(to.as_str())) else { continue };
                let end = match h {
                    mm::MessageHead::Arrow | mm::MessageHead::Open => Head::Arrow,
                    mm::MessageHead::Cross => Head::Bar,
                    mm::MessageHead::None => Head::None,
                };
                let self_msg = from == to;
                messages.push(Message {
                    from: (x0, y),
                    to: (x1, if self_msg { y + 24.0 } else { y }),
                    text: text.clone(),
                    dashed: *dashed,
                    head: end,
                    start_head: if *bidirectional { Head::Arrow } else { Head::None },
                });
                y += if self_msg { ROW + 20.0 } else { ROW };
            }
            mm::Event::Note { placement, participants, text } => {
                let xs: Vec<f64> = participants.iter().filter_map(|p| centre.get(p.as_str()).copied()).collect();
                let (Some(&lo), Some(&hi)) = (
                    xs.iter().min_by(|a, b| a.total_cmp(b)),
                    xs.iter().max_by(|a, b| a.total_cmp(b)),
                ) else {
                    continue;
                };
                let lines = text.lines().count().max(1) as f64;
                let h = (lines * 25.0 + 16.0).max(40.0);
                let want = text_w(text).width + 24.0;
                let (nx, nw) = match placement {
                    mm::NotePlacement::Over => {
                        let w = (hi - lo + 40.0).max(want).max(P_MIN_W);
                        ((lo + hi) / 2.0 - w / 2.0, w)
                    }
                    mm::NotePlacement::LeftOf => {
                        let w = want.max(P_MIN_W);
                        (lo - 10.0 - w, w)
                    }
                    mm::NotePlacement::RightOf => (hi + 10.0, want.max(P_MIN_W)),
                };
                let mut n = NodeSpec::new(format!("note:{notes}"), text.clone());
                n.size = Some([nw, h]);
                n.x = Some(nx);
                n.y = Some(y);
                n.style = Some(StyleSpec {
                    fill_color: drafftink_scene::Color::parse("#fff3bf").ok(),
                    fill_pattern: Some(FillPattern::Solid),
                    ..StyleSpec::default()
                });
                nodes.push(n);
                notes += 1;
                y += h + 16.0;
            }
        }
    }
    let y_end = y;
    let lifelines = q
        .participants
        .iter()
        .filter_map(|p| centre.get(p.id.as_str()).map(|c| (*c, P_H, y_end)))
        .collect();
    // Notes may reach beyond the participants.
    let (mut min_x, mut max_x) = (0.0f64, total_w);
    for n in &nodes {
        if let (Some(nx), Some([w, _])) = (n.x, n.size) {
            min_x = min_x.min(nx);
            max_x = max_x.max(nx + w);
        }
    }
    let bounds = Rect::new(min_x, 0.0, max_x, y_end);
    if q.participants.is_empty() {
        warnings.push("The sequence diagram has no participants.".to_owned());
    }
    let layout = LayoutOpts { algorithm: Algorithm::None, ..LayoutOpts::default() };
    let n = nodes.len();
    let built = Built {
        spec: DiagramSpec { nodes, edges: Vec::new(), groups: Vec::new(), clusters: Vec::new(), layout },
        seq: Some(Sequence { lifelines, messages, bounds }),
        group_all: group && n >= 2,
    };
    (built, warnings)
}

/// Creates the lifelines and message arrows of a sequence diagram, `shift` away from the coordinates of `seq`.
/// Messages are free arrows (a lifeline is a line, which arrows cannot bind to). Returns the ids, lifelines first.
pub(super) fn sequence_extras<T: SceneTx>(
    tx: &mut T,
    m: &dyn TextMeasure,
    ids: &mut dyn IdSource,
    seq: &Sequence,
    shift: Vec2,
) -> Result<Vec<ElementId>, super::BuildError> {
    use super::BuildError;
    let at = |(x, y): (f64, f64)| Point::new(x + shift.x, y + shift.y);
    let mut out = Vec::new();
    for &(x, y0, y1) in &seq.lifelines {
        let id = unused_element_id(tx, ids);
        let z = tx.next_top_index().map_err(|e| BuildError::Move(e.to_string()))?;
        let mut e = Element::linear(id.clone(), LinearKind::Line, z, &[at((x, y0)), at((x, y1))])
            .ok_or_else(|| BuildError::Move("a lifeline came out empty".to_owned()))?;
        e.style.stroke_style = StrokeStyle::Dashed;
        e.style.seed = ids.seed();
        tx.insert(e)?;
        out.push(id);
    }
    for msg in &seq.messages {
        let id = unused_element_id(tx, ids);
        let z = tx.next_top_index().map_err(|e| BuildError::Move(e.to_string()))?;
        let pts: Vec<Point> = if msg.from.0 == msg.to.0 {
            vec![at(msg.from), at((msg.from.0 + 40.0, msg.from.1)), at((msg.from.0 + 40.0, msg.to.1)), at(msg.to)]
        } else {
            vec![at(msg.from), at(msg.to)]
        };
        let mut e = Element::linear(id.clone(), LinearKind::Arrow, z, &pts)
            .ok_or_else(|| BuildError::Move("a message came out empty".to_owned()))?;
        if let ElementKind::Arrow(d) = &mut e.kind {
            d.start_head = msg.start_head;
            d.end_head = msg.head;
        }
        if msg.dashed {
            e.style.stroke_style = StrokeStyle::Dashed;
        }
        e.style.seed = ids.seed();
        tx.insert(e)?;
        if !msg.text.trim().is_empty() {
            create_label(tx, m, ids, &id, &msg.text, None).map_err(|e| BuildError::Move(e.to_string()))?;
        }
        out.push(id);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(text: &str) -> (Built, Vec<String>) {
        let Ok(Diagram::Flowchart(g)) = mm::parse(text) else { panic!("not a flowchart") };
        flowchart(&g, true)
    }

    #[test]
    fn a_flowchart_keeps_ids_labels_shapes_heads_and_direction() {
        let (b, w) = flow("graph LR\n A[Start] -->|go| B{Ok?}\n B -.-> C((End))\n C ==> A");
        assert_eq!(b.spec.layout.direction, Direction::Right);
        let ids: Vec<&str> = b.spec.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["A", "B", "C"]);
        assert_eq!(b.spec.nodes[1].shape, SpecShape::Diamond);
        assert_eq!(b.spec.nodes[2].shape, SpecShape::Ellipse);
        assert_eq!(b.spec.edges.len(), 3);
        assert_eq!(b.spec.edges[0].label.as_deref(), Some("go"));
        assert_eq!(b.spec.edges[0].end_head, Some(Head::Arrow));
        assert_eq!(b.spec.edges[1].style.as_ref().and_then(|s| s.stroke_style), Some(StrokeStyle::Dotted));
        assert_eq!(b.spec.edges[2].style.as_ref().and_then(|s| s.stroke_width), Some(4.0));
        assert_eq!(b.spec.groups.len(), 1, "the whole diagram is one group");
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn subgraphs_become_nested_clusters_and_unsupported_shapes_are_reported() {
        let (b, w) = flow("graph TD\n subgraph outer\n A --> B\n subgraph inner\n C --> D\n end\n end\n E[(db)] --> A");
        let c: Vec<(&str, Option<&str>, Vec<&str>)> = b
            .spec
            .clusters
            .iter()
            .map(|c| (c.id.as_str(), c.parent.as_deref(), c.members.iter().map(String::as_str).collect()))
            .collect();
        assert_eq!(c, [("outer", None, vec!["A", "B"]), ("inner", Some("outer"), vec!["C", "D"])]);
        assert!(b.spec.clusters.iter().all(|c| c.group), "group=true groups every box with what is inside");
        assert_eq!(b.spec.groups.len(), 1, "and the whole diagram");
        assert!(w.iter().any(|x| x.contains("closest shape") && x.contains("cylinder")), "{w:?}");
        assert!(!w.iter().any(|x| x.contains("subgraph")), "{w:?}");
    }

    #[test]
    fn every_mermaid_shape_maps_to_a_shape_an_arrow_can_bind_to_and_only_approximations_are_reported() {
        use mm::NodeShape as M;
        let all = [
            (M::Rectangle, "A[x]"),
            (M::Rounded, "A(x)"),
            (M::Stadium, "A([x])"),
            (M::Subroutine, "A[[x]]"),
            (M::Circle, "A((x))"),
            (M::DoubleCircle, "A(((x)))"),
            (M::Diamond, "A{x}"),
            (M::Hexagon, "A{{x}}"),
            (M::Cylinder, "A[(x)]"),
            (M::Parallelogram, "A[/x/]"),
            (M::ParallelogramAlt, "A[\\x\\]"),
            (M::Trapezoid, "A[/x\\]"),
            (M::TrapezoidAlt, "A[\\x/]"),
            (M::Asymmetric, "A>x]"),
        ];
        for (shape, src) in all {
            let (b, w) = flow(&format!("graph TD\n {src} --> B"));
            let spec = &b.spec.nodes[0];
            let drawn = drawn_shape(shape);
            assert!(matches!(spec.shape, SpecShape::Rectangle | SpecShape::Ellipse | SpecShape::Diamond), "{src}");
            assert_eq!(spec.shape, drawn.shape, "{src}");
            assert_eq!(spec.size, drawn.size, "{src}");
            assert_eq!(spec.style.as_ref().and_then(|s| s.corner_radius), drawn.corner_radius, "{src}");
            assert_eq!(w.len(), usize::from(drawn.approximates.is_some()), "{src}: {w:?}");
            if let Some(name) = drawn.approximates {
                assert!(w[0].contains(name), "{src}: {w:?}");
            }
        }
    }

    #[test]
    fn the_warning_names_each_approximated_shape_once() {
        let (_, w) = flow("graph TD\n A{{h1}} --> B{{h2}}\n B --> C[/p/]\n C --> D[(db)]\n D --> E[x]");
        assert_eq!(w.len(), 1, "{w:?}");
        for name in ["hexagon", "parallelogram", "cylinder"] {
            assert_eq!(w[0].matches(name).count(), 1, "{name} in {w:?}");
        }
        assert!(!w[0].contains("trapezoid"), "{w:?}");
    }

    #[test]
    fn approximated_shapes_look_different_from_a_plain_box_and_from_each_other() {
        use mm::NodeShape as M;
        let radius = |s| drawn_shape(s).corner_radius;
        assert_eq!(radius(M::Rectangle), None);
        assert!(radius(M::Stadium) > radius(M::Hexagon) && radius(M::Hexagon) > radius(M::Cylinder));
        assert!(radius(M::Hexagon).is_some() && radius(M::Asymmetric).is_some());
        assert!(drawn_shape(M::DoubleCircle).size > drawn_shape(M::Circle).size);
    }

    #[test]
    fn an_edge_to_a_subgraph_ends_at_its_box_and_one_box_around_everything_needs_no_outer_group() {
        let (b, _) = flow("graph TD\n subgraph s[Title]\n A --> B\n end\n s --> A");
        assert_eq!(b.spec.edges.len(), 2);
        assert_eq!((b.spec.edges[1].from.as_str(), b.spec.edges[1].to.as_str()), ("s", "A"));
        assert!(b.spec.groups.is_empty(), "the box already holds every node: {:?}", b.spec.groups);
    }

    #[test]
    fn a_sequence_diagram_has_lifelines_and_messages_below_the_participants() {
        let Ok(Diagram::Sequence(q)) = mm::parse("sequenceDiagram\n participant A\n participant B as Bob\n A->>B: hi\n B-->>B: think\n Note over A,B: shared") else {
            panic!("not a sequence")
        };
        let (b, _) = sequence(&q, true, &drafftink_edit::text_bind::MonospaceMeasure::default());
        let seq = b.seq.unwrap();
        assert_eq!(b.spec.nodes.len(), 3);
        assert_eq!(seq.lifelines.len(), 2);
        assert_eq!(seq.messages.len(), 2);
        assert!(seq.messages[0].from.1 > P_H);
        assert!(seq.messages[1].from.1 > seq.messages[0].from.1);
        assert_eq!(seq.messages[1].from.0, seq.messages[1].to.0, "a self message loops on its lifeline");
        assert!(seq.bounds.height() > seq.messages[1].from.1);
    }
}
