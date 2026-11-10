//! The plain data a Mermaid import produces. No layout, no geometry: the coordinator maps these types to the
//! diagram layout of the edit crate.

use drafftink_scene::Color;
use serde::{Deserialize, Serialize};

/// A parsed Mermaid diagram of a supported kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Diagram {
    /// `graph` or `flowchart`.
    Flowchart(MermaidGraph),
    /// `sequenceDiagram`.
    Sequence(SequenceDiagram),
}

/// Flow direction of a flowchart or of one subgraph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    /// Top to bottom (`TB`, and its alias `TD`).
    #[default]
    TopToBottom,
    /// Bottom to top (`BT`).
    BottomToTop,
    /// Left to right (`LR`).
    LeftToRight,
    /// Right to left (`RL`).
    RightToLeft,
}

/// A flowchart: nodes, edges and subgraphs, in the order they first appeared in the text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MermaidGraph {
    /// Main flow direction. `TD` is stored as [`Direction::TopToBottom`].
    pub direction: Direction,
    /// The direction word as written in the header, upper-cased (`"TD"`, `"LR"`, ...). `"TB"` when none was given.
    pub direction_token: String,
    /// Nodes in order of first appearance. Ids are unique.
    pub nodes: Vec<Node>,
    /// Edges in source order. A chain `A --> B --> C` and `A & B --> C` are expanded to single edges.
    pub edges: Vec<Edge>,
    /// Subgraphs in order of opening. `Node::subgraph`, `Subgraph::parent` and the edge endpoint indices point here.
    pub subgraphs: Vec<Subgraph>,
    /// Plain-words notes about statements that were skipped or parts that were ignored. Capped.
    pub warnings: Vec<String>,
}

/// One flowchart node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// The identifier used in the text. Not limited in length here.
    pub id: String,
    /// Display text. Real newlines replace `<br>`. The id when the node has no text of its own.
    pub label: String,
    /// Outline shape.
    pub shape: NodeShape,
    /// Index of the innermost subgraph that contains this node.
    pub subgraph: Option<usize>,
    /// Class names from `A:::name` and `class A name`, in order, without duplicates.
    pub classes: Vec<String>,
    /// Resolved colours: the `default` class, then `classDef` styles in class order, then `style` lines.
    pub style: Option<NodeStyle>,
}

/// The outline of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeShape {
    /// `A[text]`, a bare id.
    Rectangle,
    /// `A(text)`.
    Rounded,
    /// `A([text])`.
    Stadium,
    /// `A[[text]]`.
    Subroutine,
    /// `A((text))`.
    Circle,
    /// `A(((text)))`.
    DoubleCircle,
    /// `A{text}`.
    Diamond,
    /// `A{{text}}`.
    Hexagon,
    /// `A[(text)]`.
    Cylinder,
    /// `A[/text/]`.
    Parallelogram,
    /// `A[\text\]`.
    ParallelogramAlt,
    /// `A[/text\]`: the long side is at the bottom.
    Trapezoid,
    /// `A[\text/]`: the long side is at the top.
    TrapezoidAlt,
    /// `A>text]`: a flag.
    Asymmetric,
}

/// Colours from `style`, `classDef` and `class` lines. A colour that does not parse is dropped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeStyle {
    /// `fill`.
    pub fill: Option<Color>,
    /// `stroke`.
    pub stroke: Option<Color>,
    /// `stroke-width`, in pixels. Finite and not negative.
    pub stroke_width: Option<f64>,
    /// `color`: the text colour.
    pub color: Option<Color>,
}

impl NodeStyle {
    /// True when no property is set.
    pub fn is_empty(&self) -> bool {
        self.fill.is_none() && self.stroke.is_none() && self.stroke_width.is_none() && self.color.is_none()
    }

    /// Sets every property that `over` has, keeping the others.
    pub fn merge(&mut self, over: &NodeStyle) {
        self.fill = over.fill.or(self.fill);
        self.stroke = over.stroke.or(self.stroke);
        self.stroke_width = over.stroke_width.or(self.stroke_width);
        self.color = over.color.or(self.color);
    }
}

/// The stroke of an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Line {
    /// `-->`, `---`.
    Solid,
    /// `-.->`, `-.-`.
    Dotted,
    /// `==>`, `===`. Thick links stay thick.
    Thick,
    /// `~~~`: takes part in the layout but is not drawn.
    Invisible,
}

/// The end of an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Head {
    /// No head.
    None,
    /// `>` or `<`.
    Arrow,
    /// `x`.
    Cross,
    /// `o`.
    Circle,
}

/// One edge between two nodes (or subgraphs).
///
/// Mermaid allows a subgraph id as an endpoint. Then `from_subgraph` (or `to_subgraph`) is the index into
/// [`MermaidGraph::subgraphs`], and `from` (or `to`) holds the subgraph id, which is not the id of any node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    /// Source node id, or subgraph id when `from_subgraph` is set.
    pub from: String,
    /// Target node id, or subgraph id when `to_subgraph` is set.
    pub to: String,
    /// Set when `from` names a subgraph.
    pub from_subgraph: Option<usize>,
    /// Set when `to` names a subgraph.
    pub to_subgraph: Option<usize>,
    /// Text on the edge.
    pub label: Option<String>,
    /// Stroke.
    pub line: Line,
    /// Head at the `from` end (`<--`, `x--x`, `o--o`).
    pub head_start: Head,
    /// Head at the `to` end.
    pub head_end: Head,
}

/// A `subgraph ... end` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subgraph {
    /// Identifier. Derived from the title when the text gave none.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Index of the enclosing subgraph.
    pub parent: Option<usize>,
    /// From a `direction XX` line inside the block.
    pub direction: Option<Direction>,
    /// Ids of the nodes whose innermost subgraph this is. Nodes of nested subgraphs are listed there.
    pub members: Vec<String>,
}

/// A sequence diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SequenceDiagram {
    /// Participants in order of declaration or first use.
    pub participants: Vec<Participant>,
    /// Messages and notes in source order.
    pub events: Vec<Event>,
    /// Plain-words notes about ignored parts. Capped.
    pub warnings: Vec<String>,
}

/// One lifeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    /// Identifier used in messages.
    pub id: String,
    /// Display text (`participant X as Label`), the id when there is none.
    pub label: String,
    /// Declared with `actor`.
    pub is_actor: bool,
}

/// The arrowhead of a sequence message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageHead {
    /// `->>`, `-->>`: filled arrowhead.
    Arrow,
    /// `-x`, `--x`: cross.
    Cross,
    /// `-)`, `--)`: open arrowhead (asynchronous).
    Open,
    /// `->`, `-->`: a line without a head.
    None,
}

/// The `+` or `-` right after a message arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Activation {
    /// None given.
    None,
    /// `+`: the receiver becomes active.
    Activate,
    /// `-`: the sender becomes inactive.
    Deactivate,
}

/// Where a note sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotePlacement {
    /// `Note over A` or `Note over A,B`.
    Over,
    /// `Note left of A`.
    LeftOf,
    /// `Note right of A`.
    RightOf,
}

/// Something that happens in a sequence diagram.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// An arrow between two participants (equal ids make a self message).
    Message {
        /// Sender id.
        from: String,
        /// Receiver id.
        to: String,
        /// Text after the colon.
        text: String,
        /// Dashed line (`-->>`, `--x`, ...).
        dashed: bool,
        /// Head at the receiver.
        head: MessageHead,
        /// A head at both ends (`<<->>`, `<<-->>`).
        bidirectional: bool,
        /// Activation marker after the arrow.
        activation: Activation,
    },
    /// A note.
    Note {
        /// Placement.
        placement: NotePlacement,
        /// Participant ids: one, or up to two for `over`.
        participants: Vec<String>,
        /// Note text, `<br>` turned into newlines.
        text: String,
    },
}
