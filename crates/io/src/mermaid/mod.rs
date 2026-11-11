//! Mermaid import: text in, plain data out. Flowcharts (`graph`, `flowchart`) and sequence diagrams.
//!
//! This module only parses. It produces a neutral, serde-serializable graph ([`MermaidGraph`],
//! [`SequenceDiagram`]) and does no layout and no drawing: the coordinator maps the graph to the diagram layout of
//! the edit crate. The module is pure std + serde and builds for `wasm32`.
//!
//! # Entry points
//! [`parse`] detects the diagram type. [`parse_flowchart`] and [`parse_sequence`] insist on one type and return
//! [`IoError::WrongFormat`] for the other. Any other Mermaid type (class, state, ER, gantt, pie, ...) and plain
//! text give [`IoError::WrongFormat`] with a hint that names the supported types. A diagram without any node (or
//! participant) is [`IoError::Invalid`].
//!
//! # Preparation
//! * Input larger than [`crate::limits::MAX_INPUT_BYTES`] and more than [`crate::limits::MAX_DIAGRAM_LINES`] lines
//!   are refused with [`IoError::Limit`].
//! * A leading code fence (three backticks or tildes with an optional tag) and its closing fence are removed.
//! * A front-matter block (`---` ... `---`, title or config) at the start is skipped, before or inside a fence.
//! * `%% comment` lines and `%%{init: ...}%%` directives (also over several lines) are comments.
//! * The header is the first line that is neither empty nor a comment. Its first word decides the type, compared
//!   as a whole word and case-sensitively: `graph`, `flowchart` and `flowchart-elk` give a flowchart,
//!   `sequenceDiagram` a sequence diagram. (`graphX` and `Graph` no longer match; the old importer matched a prefix.)
//!
//! # Flowchart decisions
//! * Statements are separated by newlines and by `;` outside quotes and brackets. `%%` starts a comment outside
//!   quotes and brackets. Statements may follow the header on the same line (`graph TD; A-->B`).
//! * `A & B --> C & D` expands to the full cross product. A product that would exceed
//!   [`crate::limits::MAX_DIAGRAM_ITEMS`] nodes plus edges plus subgraphs is refused with [`IoError::Limit`]
//!   before any edge is allocated. Together, ids and labels may hold at most [`MAX_TOTAL_TEXT_BYTES`]; ids at most
//!   [`MAX_ID_CHARS`] chars; labels are cut at [`MAX_LABEL_CHARS`] chars; subgraphs nest at most
//!   [`MAX_SUBGRAPH_DEPTH`] deep. Parsing is a loop over lines with an explicit stack, never recursion.
//! * A statement that cannot be understood is skipped and reported in `warnings`; the rest still imports.
//! * Node ids are runs of Unicode letters, digits and `_` (as before). A node that is defined twice takes the last
//!   explicit shape and label (this is Mermaid's own rule; the old importer kept the first). A node belongs to the
//!   subgraph in which it is first referenced inside a subgraph; a node that was first used outside any subgraph
//!   moves into the first subgraph (or a deeper one) that mentions it.
//! * Labels: surrounding double quotes and markdown backticks are removed, `<br>`, `<br/>`, `<br />` become a
//!   newline, `#quot;`, `#35;` (decimal), `&amp;`, `&lt;`, `&gt;`, `&apos;`, `&nbsp;`, `&#65;` and `&#x41;` are decoded,
//!   control characters are dropped, and the text is trimmed. An empty label shows the id.
//! * Links: `-->` `---` `-.->` `-.-` `==>` `===` `~~~` `--x` `--o` `<-->` `x--x` `o--o`, longer forms (`---->`,
//!   `-..->`, `====>`), text with a pipe (`-->|text|`) and embedded text (`-- text -->`, `-. text .->`,
//!   `== text ==>`). `->` is accepted as an arrow for compatibility. `<--` alone gives a start head only. Edge ids
//!   (`A e1@--> B`) are accepted and dropped. As in Mermaid, `x` and `o` right after the dashes are heads: write
//!   `A --- oB` with a space to get a node `oB`.
//! * A link may name a subgraph id. Then [`Edge::from_subgraph`] or [`Edge::to_subgraph`] holds the index, and the
//!   id is not the id of any node. A node that exists only because such a link named it is dropped.
//! * Shapes: see [`NodeShape`]. `A@{ shape: cyl, label: "x" }` is accepted for the basic keys `shape` and `label`
//!   (short names like `rect`, `rounded`, `stadium`, `diam`, `hex`, `cyl`, `circle`, `dbl-circ`, `lean-r`,
//!   `lean-l`, `trap-b`, `trap-t`, `odd`, `fr-rect`); other keys and unknown shapes are ignored (a warning for
//!   unknown shapes). `e1@{ animate: true }` is ignored without creating a node.
//! * `classDef`, `class`, `style` and `A:::name` fill [`Node::classes`] and [`Node::style`] (`fill`, `stroke`,
//!   `stroke-width`, `color`; colours through `drafftink_scene::Color::parse`, so only hex colours and
//!   `transparent` work, others are dropped). Order of precedence: `classDef default`, the node's classes in order,
//!   then `style` lines. They may appear before or after the nodes they name.
//! * `click`, `linkStyle`, `accTitle`, `accDescr` (also the block form) are accepted and ignored.
//! * `direction XX` inside a subgraph sets [`Subgraph::direction`]; outside a subgraph it is ignored.
//!
//! # Sequence decisions
//! * Block keywords (`autonumber`, `title`, `loop`, `alt`, `else`, `opt`, `par`, `and`, `critical`, `option`,
//!   `break`, `rect`, `box`, `end`, `activate`, `deactivate`, `destroy`, `link`, `links`, `properties`, `details`)
//!   are recognised first and skipped, so `alt x -> y` is no message. Messages inside blocks are kept.
//!   `create participant X` declares the participant.
//! * Arrow tokens: `->>` `-->>` `->` `-->` `-x` `--x` `-)` `--)` `<<->>` `<<-->>`. The arrow is searched only left
//!   of the first `:`, so the text may contain arrows. A `+` or `-` right after the arrow is stripped and recorded
//!   in [`Event::Message::activation`].
//! * Unknown participants in messages and notes are added in order of first use.

mod flowchart;
mod sequence;
mod text;
mod types;

pub use types::*;

use crate::error::{Expected, FormatName, IoError};
use crate::limits::{MAX_DIAGRAM_LINES, MAX_INPUT_BYTES};

/// Longest label kept, in chars. Longer text is cut.
pub const MAX_LABEL_CHARS: usize = 10_000;
/// Deepest subgraph nesting accepted.
pub const MAX_SUBGRAPH_DEPTH: usize = 32;
/// Longest node, subgraph or participant id accepted, in chars. A longer one is refused, not cut, because cutting
/// would merge different ids.
pub const MAX_ID_CHARS: usize = 1_000;
/// Most bytes of ids and labels a diagram may hold in total. Stops a short input from expanding (a label on a cross
/// product is copied to every edge).
pub const MAX_TOTAL_TEXT_BYTES: usize = 64 * 1024 * 1024;

const HINT: &str = "only flowcharts (graph, flowchart) and sequenceDiagram can be imported";

/// Parses Mermaid text of a supported type.
///
/// # Errors
/// [`IoError::WrongFormat`] for another Mermaid type or for text that is no diagram, [`IoError::Limit`] for input
/// beyond a limit, [`IoError::Invalid`] for a diagram without content, [`IoError::Parse`] for an empty text or an
/// unclosed front matter.
pub fn parse(text: &str) -> Result<Diagram, IoError> {
    let p = prepare(text)?;
    match p.kind {
        Kind::Flowchart => flowchart::parse(&p).map(Diagram::Flowchart),
        Kind::Sequence => sequence::parse(&p).map(Diagram::Sequence),
    }
}

/// Parses a flowchart (`graph` or `flowchart`).
///
/// # Errors
/// As [`parse`]. A sequence diagram gives [`IoError::WrongFormat`].
pub fn parse_flowchart(text: &str) -> Result<MermaidGraph, IoError> {
    let p = prepare(text)?;
    match p.kind {
        Kind::Flowchart => flowchart::parse(&p),
        Kind::Sequence => Err(IoError::WrongFormat {
            expected: Expected::MermaidFlowchart,
            hint: "this is a sequenceDiagram; use parse or parse_sequence".to_string(),
        }),
    }
}

/// Parses a `sequenceDiagram`.
///
/// # Errors
/// As [`parse`]. A flowchart gives [`IoError::WrongFormat`].
pub fn parse_sequence(text: &str) -> Result<SequenceDiagram, IoError> {
    let p = prepare(text)?;
    match p.kind {
        Kind::Sequence => sequence::parse(&p),
        Kind::Flowchart => Err(IoError::WrongFormat {
            expected: Expected::MermaidSequence,
            hint: "this is a flowchart; use parse or parse_flowchart".to_string(),
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Flowchart,
    Sequence,
}

/// The text after fence and front-matter removal.
pub(crate) struct Prepared<'a> {
    kind: Kind,
    /// The header line after its first word, trimmed.
    header_rest: &'a str,
    /// The lines after the header, without comment-only lines and init directives.
    lines: Vec<&'a str>,
}

fn is_fence(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("```") || t.starts_with("~~~")
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn prepare(text: &str) -> Result<Prepared<'_>, IoError> {
    if text.len() > MAX_INPUT_BYTES {
        return Err(IoError::Limit { what: "Mermaid text", max: format!("{MAX_INPUT_BYTES} bytes") });
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut all: Vec<&str> = Vec::new();
    for line in text.lines() {
        if all.len() >= MAX_DIAGRAM_LINES {
            return Err(IoError::Limit { what: "Mermaid text", max: format!("{MAX_DIAGRAM_LINES} lines") });
        }
        all.push(line);
    }

    let mut start = 0usize;
    let mut end = all.len();
    skip_blank(&all, &mut start, end);
    skip_front_matter(&all, &mut start, end)?;
    skip_blank(&all, &mut start, end);
    let fenced = all.get(start).is_some_and(|l| is_fence(l));
    if fenced {
        start += 1;
        skip_blank(&all, &mut start, end);
        skip_front_matter(&all, &mut start, end)?;
        while end > start && all.get(end - 1).is_some_and(|l| is_blank(l)) {
            end -= 1;
        }
        if end > start && all.get(end - 1).is_some_and(|l| is_fence(l)) {
            end -= 1;
        }
    }

    let mut header: Option<&str> = None;
    let mut lines: Vec<&str> = Vec::new();
    let mut in_directive = false;
    for line in all.get(start..end).unwrap_or(&[]) {
        let t = line.trim();
        if in_directive {
            in_directive = !t.contains("}%%");
            continue;
        }
        if t.starts_with("%%{") && !t.contains("}%%") {
            in_directive = true;
            continue;
        }
        if t.starts_with("%%") {
            continue;
        }
        if header.is_none() {
            if !t.is_empty() {
                header = Some(t);
            }
            continue;
        }
        lines.push(line);
    }
    let Some(header) = header else {
        return Err(IoError::parse(FormatName::Mermaid, "the text is empty"));
    };
    let word_end = header.find(|c: char| c.is_whitespace() || c == ';').unwrap_or(header.len());
    let word = header.get(..word_end).unwrap_or("");
    let header_rest = header.get(word_end..).unwrap_or("").trim_start();
    let kind = match word {
        "graph" | "flowchart" | "flowchart-elk" => Kind::Flowchart,
        "sequenceDiagram" => Kind::Sequence,
        _ => {
            let hint = if looks_like_type(word) {
                format!("the diagram type `{word}` is not supported; {HINT}")
            } else {
                format!("the text does not start with a Mermaid diagram type; {HINT}")
            };
            return Err(IoError::WrongFormat { expected: Expected::MermaidAny, hint });
        }
    };
    Ok(Prepared { kind, header_rest, lines })
}

/// True for the first word of the other Mermaid diagram types, for a better hint.
fn looks_like_type(word: &str) -> bool {
    const KNOWN: &[&str] = &[
        "classDiagram",
        "classDiagram-v2",
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
        "C4Context",
        "sankey-beta",
        "xychart-beta",
        "block-beta",
        "packet-beta",
        "architecture-beta",
        "kanban",
        "zenuml",
    ];
    KNOWN.contains(&word)
}

fn skip_blank(all: &[&str], start: &mut usize, end: usize) {
    while *start < end && all.get(*start).is_some_and(|l| is_blank(l)) {
        *start += 1;
    }
}

/// Skips a `---` ... `---` block at `start`. An opening without closing is an error.
fn skip_front_matter(all: &[&str], start: &mut usize, end: usize) -> Result<(), IoError> {
    if !all.get(*start).is_some_and(|l| l.trim() == "---") {
        return Ok(());
    }
    let mut i = *start + 1;
    while i < end {
        if all.get(i).is_some_and(|l| l.trim() == "---") {
            *start = i + 1;
            return Ok(());
        }
        i += 1;
    }
    Err(IoError::parse(FormatName::Mermaid, "the front matter starts with --- but is never closed"))
}

/// Pushes a warning unless the cap is reached.
pub(crate) fn push_warning(warnings: &mut Vec<String>, message: String) {
    if warnings.len() < crate::limits::MAX_WARNINGS {
        warnings.push(message);
    }
}
