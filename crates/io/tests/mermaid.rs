//! Mermaid import: the fixtures of the old importer, regression tests for its bugs, limits and hostile input.

// Integration tests run natively: `Instant` (time bounds) is fine here, and a failing test may panic.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use drafftink_io::IoError;
use drafftink_io::limits::{MAX_DIAGRAM_ITEMS, MAX_DIAGRAM_LINES};
use drafftink_io::mermaid::{
    Activation, Diagram, Direction, Event, Head, Line, MAX_LABEL_CHARS, MAX_SUBGRAPH_DEPTH, MermaidGraph, MessageHead,
    NodeShape, NotePlacement, SequenceDiagram, parse, parse_flowchart, parse_sequence,
};
use drafftink_scene::Color;
use proptest::prelude::*;
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixtures().join(rel)).unwrap()
}

fn g(src: &str) -> MermaidGraph {
    parse_flowchart(src).unwrap_or_else(|e| panic!("{src:?}: {e}"))
}

fn seq(src: &str) -> SequenceDiagram {
    parse_sequence(src).unwrap_or_else(|e| panic!("{src:?}: {e}"))
}

/// `(from, to)` of every edge.
fn pairs(g: &MermaidGraph) -> Vec<(&str, &str)> {
    g.edges.iter().map(|e| (e.from.as_str(), e.to.as_str())).collect()
}

fn node_ids(g: &MermaidGraph) -> Vec<&str> {
    g.nodes.iter().map(|n| n.id.as_str()).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Fixtures

#[test]
fn every_fixture_is_classified_like_the_old_importer() {
    let summary: Value = serde_json::from_str(&read("mermaid/expected/summary.json")).unwrap();
    let inputs = summary["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 27);
    for item in inputs {
        let input = item["input"].as_str().unwrap();
        let accepted = item["accepted"].as_bool().unwrap();
        let result = parse(&read(input));
        assert_eq!(result.is_ok(), accepted, "{input}: {result:?}");
    }
}

#[test]
fn rejected_inputs_say_why() {
    for (name, wrong_format) in [
        ("class_diagram", true),
        ("er_diagram", true),
        ("gantt", true),
        ("pie", true),
        ("state_diagram", true),
        ("prose", true),
        ("lowercase_graph", true),
        ("empty", false),
        ("graph_without_content", false),
        ("sequence_without_participants", false),
    ] {
        let err = parse(&read(&format!("mermaid/rejected/{name}.mmd"))).unwrap_err();
        match (&err, wrong_format) {
            (IoError::WrongFormat { hint, .. }, true) => {
                assert!(hint.contains("flowcharts") && hint.contains("sequenceDiagram"), "{name}: {hint}");
            }
            (IoError::Invalid { .. } | IoError::Parse { .. }, false) => {}
            _ => panic!("{name}: unexpected error {err:?}"),
        }
    }
}

/// The old output as leaf shapes: `(kind, value)`, groups flattened.
fn old_leaves(name: &str) -> Vec<(String, Value)> {
    fn walk(shape: &Value, out: &mut Vec<(String, Value)>) {
        let (kind, v) = shape.as_object().unwrap().iter().next().unwrap();
        if kind == "Group" {
            for c in v["children"].as_array().unwrap() {
                walk(c, out);
            }
        } else {
            out.push((kind.clone(), v.clone()));
        }
    }
    let doc: Value = serde_json::from_str(&read(&format!("mermaid/expected/{name}.json"))).unwrap();
    let mut out = Vec::new();
    for s in doc["shapes"].as_object().unwrap().values() {
        walk(s, &mut out);
    }
    out
}

fn old_texts(leaves: &[(String, Value)]) -> Vec<String> {
    let mut t: Vec<String> =
        leaves.iter().filter(|(k, _)| k == "Text").map(|(_, v)| v["content"].as_str().unwrap().to_string()).collect();
    t.sort();
    t
}

/// What is compared for flowcharts (the old output is a drawing, so the comparison is on what the drawing shows):
/// * the multiset of all texts: node labels, edge labels and subgraph titles;
/// * the number of arrows plus open lines (unclosed `Line`, minus the two insets of each subroutine) = edges
///   without the invisible ones;
/// * the multiset of (has a head, dotted) per edge.
///
/// Fixtures where the old importer had a bug are listed below and asserted separately (`old_bug_fixtures_are_read_correctly`,
/// `flowchart_fixture_details`): the old text set was wrong for `TEXT_BUGS`, the old edge set for `EDGE_BUGS`.
const TEXT_BUGS: &[&str] = &["flowchart_comment_before_header", "flowchart_cycle", "flowchart_quoted_labels"];
const EDGE_BUGS: &[&str] = &["flowchart_quoted_labels", "flowchart_chain", "flowchart_large"];

fn new_flowchart_texts(g: &MermaidGraph) -> Vec<String> {
    let mut t: Vec<String> = g.nodes.iter().map(|n| n.label.clone()).collect();
    t.extend(g.edges.iter().filter_map(|e| e.label.clone()));
    t.extend(g.subgraphs.iter().map(|s| s.title.clone()));
    t.sort();
    t
}

fn old_edge_kinds(leaves: &[(String, Value)], subroutines: usize) -> Vec<(bool, bool)> {
    let mut v = Vec::new();
    let mut open_lines = 0;
    for (k, s) in leaves {
        let dotted = s["stroke_style"] == "Dotted";
        match k.as_str() {
            "Arrow" => v.push((true, dotted)),
            "Line" if s["closed"] == false => {
                open_lines += 1;
                v.push((false, dotted));
            }
            _ => {}
        }
    }
    // Remove the subroutine insets (solid, no head).
    for _ in 0..subroutines * 2 {
        let pos = v.iter().position(|e| *e == (false, false)).unwrap();
        v.remove(pos);
        open_lines -= 1;
    }
    assert!(open_lines >= 0);
    v.sort();
    v
}

fn new_edge_kinds(g: &MermaidGraph) -> Vec<(bool, bool)> {
    let mut v: Vec<(bool, bool)> = g
        .edges
        .iter()
        .filter(|e| e.line != Line::Invisible)
        .map(|e| (e.head_start != Head::None || e.head_end != Head::None, e.line == Line::Dotted))
        .collect();
    v.sort();
    v
}

#[test]
fn flowchart_fixtures_match_the_old_output() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures().join("mermaid")).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
        if path.extension().and_then(|e| e.to_str()) != Some("mmd") || name.starts_with("sequence_") {
            continue;
        }
        let Diagram::Flowchart(graph) = parse(&read(&format!("mermaid/{name}.mmd"))).unwrap() else { continue };
        let leaves = old_leaves(&name);
        if !TEXT_BUGS.contains(&name.as_str()) {
            assert_eq!(new_flowchart_texts(&graph), old_texts(&leaves), "{name}: texts");
        }
        if !EDGE_BUGS.contains(&name.as_str()) {
            let subroutines = graph.nodes.iter().filter(|n| n.shape == NodeShape::Subroutine).count();
            assert_eq!(new_edge_kinds(&graph), old_edge_kinds(&leaves, subroutines), "{name}: edges");
        }
        checked += 1;
    }
    assert_eq!(checked, 12);
}

#[test]
fn flowchart_fixture_details() {
    // Direction.
    assert_eq!(g(&read("mermaid/flowchart_directions_bt.mmd")).direction, Direction::BottomToTop);
    assert_eq!(g(&read("mermaid/flowchart_directions_rl.mmd")).direction, Direction::RightToLeft);
    let chain = g(&read("mermaid/flowchart_chain.mmd"));
    assert_eq!(chain.direction, Direction::LeftToRight);
    // Chain: A-->B-->C & D, D-->E, E-->A. Nodes in order of appearance.
    assert_eq!(node_ids(&chain), ["A", "B", "C", "D", "E"]);
    assert_eq!(pairs(&chain), [("A", "B"), ("B", "C"), ("B", "D"), ("D", "E"), ("E", "A")]);
    // classDef / class / style are applied (the old importer ignored them).
    let a = &chain.nodes[0];
    assert_eq!(a.classes, ["green"]);
    let st = a.style.as_ref().unwrap();
    assert_eq!(st.fill, Some(Color::rgb(0x99, 0xff, 0x66)));
    assert_eq!(st.stroke, Some(Color::rgb(0x33, 0x33, 0x33)));
    assert_eq!(chain.nodes[1].style.as_ref().unwrap().fill, Some(Color::rgb(0xff, 0x99, 0xff)));
    assert!(chain.nodes[2].style.is_none());

    // Edges: every form of flowchart_edges.mmd.
    let e = g(&read("mermaid/flowchart_edges.mmd"));
    let kinds: Vec<(Line, Head, Head, Option<&str>)> =
        e.edges.iter().map(|x| (x.line, x.head_start, x.head_end, x.label.as_deref())).collect();
    assert_eq!(
        kinds,
        [
            (Line::Solid, Head::None, Head::None, None),
            (Line::Solid, Head::None, Head::Arrow, None),
            (Line::Dotted, Head::None, Head::Arrow, None),
            (Line::Thick, Head::None, Head::Arrow, None),
            (Line::Solid, Head::Arrow, Head::Arrow, None),
            (Line::Solid, Head::None, Head::Arrow, Some("pipe label")),
            (Line::Solid, Head::None, Head::Arrow, Some("embedded label")),
            (Line::Dotted, Head::None, Head::Arrow, Some("dotted label")),
            (Line::Solid, Head::None, Head::Cross, None),
            (Line::Solid, Head::None, Head::Circle, None),
        ]
    );

    // Shapes.
    let s = g(&read("mermaid/flowchart_shapes.mmd"));
    let shapes: Vec<NodeShape> = s.nodes.iter().map(|n| n.shape).collect();
    assert_eq!(
        shapes,
        [
            NodeShape::Rectangle,
            NodeShape::Rounded,
            NodeShape::Diamond,
            NodeShape::Circle,
            NodeShape::Stadium,
            NodeShape::Subroutine,
            NodeShape::Hexagon,
            NodeShape::Rectangle,
        ]
    );
    assert_eq!(s.nodes[7].label, "Multi\nline label");

    // Subgraphs.
    let sg = g(&read("mermaid/flowchart_subgraph.mmd"));
    assert_eq!(sg.subgraphs.len(), 3);
    assert_eq!((sg.subgraphs[0].id.as_str(), sg.subgraphs[0].title.as_str()), ("KITCHEN", "The order pipeline"));
    assert_eq!(sg.subgraphs[0].direction, Some(Direction::LeftToRight));
    assert_eq!(sg.subgraphs[0].members, ["A", "B", "C"]);
    assert_eq!(sg.subgraphs[2].id, "NESTED");
    assert_eq!(sg.subgraphs[2].parent, Some(1));
    assert_eq!(sg.subgraphs[1].members, ["R1", "R2"]);
    assert_eq!(sg.subgraphs[2].members, ["N1", "N2"]);
    // `KITCHEN --> REAL` ends at subgraphs, and no node named KITCHEN or REAL is left.
    assert!(!node_ids(&sg).contains(&"KITCHEN") && !node_ids(&sg).contains(&"REAL"));
    let to_sg = sg.edges.iter().find(|x| x.from == "KITCHEN").unwrap();
    assert_eq!((to_sg.from_subgraph, to_sg.to_subgraph), (Some(0), Some(1)));
    let serve = sg.edges.iter().find(|x| x.label.as_deref() == Some("serve")).unwrap();
    assert_eq!((serve.from_subgraph, serve.to_subgraph, serve.line), (None, None, Line::Dotted));

    // Unicode and fenced.
    let u = g(&read("mermaid/unicode_labels.mmd"));
    let labels: Vec<&str> = u.nodes.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(labels, ["日本語", "Ünïcödé ✓", "😀 emoji"]);
    let f = g(&read("mermaid/flowchart_fenced.mmd"));
    assert_eq!(f.nodes.len(), 4);
    assert_eq!(f.nodes[1].shape, NodeShape::Diamond);
    let large = g(&read("mermaid/flowchart_large.mmd"));
    assert_eq!(large.nodes.len(), 11);
    // S --> A1 & A2 & A3 is three edges (the old importer kept one), and the rest is as before: 14 in all.
    assert_eq!(large.edges.len(), 14);
    assert_eq!(&pairs(&large)[..3], &[("S", "A1"), ("S", "A2"), ("S", "A3")]);
}

/// The three fixtures where the old importer was wrong. The new output is the correct reading.
#[test]
fn old_bug_fixtures_are_read_correctly() {
    // A comment before the header made the header a node called `graph`.
    let c = g(&read("mermaid/flowchart_comment_before_header.mmd"));
    assert_eq!(node_ids(&c), ["A", "B"]);
    assert_eq!(c.direction, Direction::TopToBottom);
    assert_eq!(c.direction_token, "TD");
    // `[(Database)]` is a cylinder, not a rectangle labelled `(Database)`.
    let cy = g(&read("mermaid/flowchart_cycle.mmd"));
    let db = cy.nodes.iter().find(|n| n.id == "C").unwrap();
    assert_eq!((db.shape, db.label.as_str()), (NodeShape::Cylinder, "Database"));
    assert_eq!(cy.edges.len(), 5);
    assert!(cy.edges.iter().any(|e| e.from == "B" && e.to == "B"));
    assert_eq!(cy.edges[0].label.as_deref(), Some("request"));
    // The label with brackets inside quotes stays whole, and `<br>` variants become newlines.
    let q = g(&read("mermaid/flowchart_quoted_labels.mmd"));
    assert_eq!(q.nodes[0].label, "quoted: label (with) [brackets]");
    assert_eq!(q.nodes[1].label, "two\nlines\nhere");
    assert_eq!((q.nodes[2].shape, q.nodes[2].label.as_str()), (NodeShape::Rounded, "rounded \n break"));
    assert_eq!(q.edges.len(), 2);
}

/// Sequence fixtures: compared on the texts shown (messages, notes, participant labels), on the number of arrows
/// (one per message) and on lifelines (one `Line` per participant).
#[test]
fn sequence_fixtures_match_the_old_output() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures().join("mermaid")).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
        if path.extension().and_then(|e| e.to_str()) != Some("mmd") || !name.starts_with("sequence_") {
            continue;
        }
        let d = seq(&read(&format!("mermaid/{name}.mmd")));
        let leaves = old_leaves(&name);
        let mut texts: Vec<String> = d.participants.iter().map(|p| p.label.clone()).collect();
        let mut messages = 0;
        let mut notes = 0;
        for ev in &d.events {
            match ev {
                Event::Message { text, .. } => {
                    messages += 1;
                    texts.push(text.clone());
                }
                Event::Note { text, .. } => {
                    notes += 1;
                    texts.push(text.clone());
                }
            }
        }
        texts.sort();
        assert_eq!(texts, old_texts(&leaves), "{name}: texts");
        let count = |k: &str| leaves.iter().filter(|(kind, _)| kind == k).count();
        assert_eq!(count("Arrow"), messages, "{name}: arrows");
        assert_eq!(count("Line"), d.participants.len(), "{name}: lifelines");
        assert_eq!(count("Rectangle"), d.participants.len() + notes, "{name}: boxes");
        checked += 1;
    }
    assert_eq!(checked, 5);
}

#[test]
fn sequence_fixture_details() {
    let a = seq(&read("mermaid/sequence_arrows.mmd"));
    let got: Vec<(bool, MessageHead, bool)> = a
        .events
        .iter()
        .map(|e| match e {
            Event::Message { dashed, head, bidirectional, .. } => (*dashed, *head, *bidirectional),
            Event::Note { .. } => panic!("note"),
        })
        .collect();
    assert_eq!(
        got,
        [
            (false, MessageHead::Arrow, false),
            (true, MessageHead::Arrow, false),
            (false, MessageHead::Cross, false),
            (true, MessageHead::Cross, false),
            (false, MessageHead::Open, false),
            (true, MessageHead::Open, false),
            (false, MessageHead::Arrow, true),
            (true, MessageHead::Arrow, true),
        ]
    );
    let b = seq(&read("mermaid/sequence_basic.mmd"));
    assert_eq!(
        b.participants.iter().map(|p| (p.id.as_str(), p.label.as_str())).collect::<Vec<_>>(),
        [("C", "Customer"), ("S", "Shop")]
    );
    assert!(matches!(&b.events[2], Event::Message { head: MessageHead::None, dashed: false, .. }));
    let i = seq(&read("mermaid/sequence_implicit_participants.mmd"));
    assert_eq!(i.participants.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["Alice", "Bob", "Carol"]);
    let n = seq(&read("mermaid/sequence_notes_self.mmd"));
    assert!(n.participants[0].is_actor && !n.participants[1].is_actor);
    assert_eq!(n.participants[0].label, "User");
    assert!(matches!(
        &n.events[0],
        Event::Note { placement: NotePlacement::Over, participants, text } if participants == &["U", "A"] && text == "spans two"
    ));
    assert!(matches!(&n.events[1], Event::Note { placement: NotePlacement::LeftOf, .. }));
    assert!(matches!(
        &n.events[2],
        Event::Note { placement: NotePlacement::RightOf, text, .. } if text == "right note\ntwo lines"
    ));
    assert!(
        matches!(&n.events[3], Event::Note { placement: NotePlacement::Over, participants, .. } if participants == &["A"])
    );
    assert!(matches!(&n.events[4], Event::Message { from, to, .. } if from == "A" && to == "A"));
    let u = seq(&read("mermaid/sequence_unsupported.mmd"));
    let texts: Vec<&str> = u
        .events
        .iter()
        .map(|e| match e {
            Event::Message { text, .. } => text.as_str(),
            Event::Note { .. } => "",
        })
        .collect();
    assert_eq!(texts, ["ping", "pong", "error", "after", "inline comment"]);
}

// ---------------------------------------------------------------------------------------------------------------
// Regression tests for the bugs of the old importer (docs/formats.md section 7)

#[test]
fn ampersand_expands_to_the_cross_product() {
    let a = g("graph TD\nA & B --> C");
    assert_eq!(node_ids(&a), ["A", "B", "C"]);
    assert_eq!(pairs(&a), [("A", "C"), ("B", "C")]);
    let b = g("graph TD\nA --> B & C");
    assert_eq!(node_ids(&b), ["A", "B", "C"]);
    assert_eq!(pairs(&b), [("A", "B"), ("A", "C")]);
    let c = g("graph TD\nA & B --> C & D");
    assert_eq!(node_ids(&c), ["A", "B", "C", "D"]);
    assert_eq!(pairs(&c), [("A", "C"), ("A", "D"), ("B", "C"), ("B", "D")]);
    let d = g("graph TD\nA[x] & B(y) --> C & D --> E");
    assert_eq!(d.edges.len(), 4 + 2);
    assert_eq!(d.nodes[1].shape, NodeShape::Rounded);
}

#[test]
fn comments_and_blank_lines_before_the_header() {
    let c = g("\n\n%% one\n%% two\n\nflowchart LR\n  A --> B\n");
    assert_eq!(node_ids(&c), ["A", "B"]);
    assert_eq!(c.direction, Direction::LeftToRight);
    // An init directive over several lines is a comment too.
    let d = g("%%{init: {\n  'theme': 'dark'\n}}%%\ngraph TD\nA-->B");
    assert_eq!(node_ids(&d), ["A", "B"]);
}

#[test]
fn semicolon_separates_statements_only_outside_labels() {
    let a = g("graph TD\nA[a;b] --> B; B --> C");
    assert_eq!(a.nodes[0].label, "a;b");
    assert_eq!(pairs(&a), [("A", "B"), ("B", "C")]);
    let q = g("graph TD\nA[\"x; y\"] --> B");
    assert_eq!(q.nodes[0].label, "x; y");
    let p = g("graph TD\nA(a;b) --> B{c;d}");
    assert_eq!((p.nodes[0].label.as_str(), p.nodes[1].label.as_str()), ("a;b", "c;d"));
    let e = g("graph TD\nA -->|a;b| B");
    assert_eq!(e.edges[0].label.as_deref(), Some("a;b"));
    let m = g("graph TD\nA-->B;;C-->D;");
    assert_eq!(m.edges.len(), 2);
    // `%%` in a label is not a comment.
    let c = g("graph TD\nA[\"100%% sure\"] --> B %% real comment");
    assert_eq!(c.nodes[0].label, "100%% sure");
    assert_eq!(c.edges.len(), 1);
}

#[test]
fn header_token_forms() {
    for (src, dir, token) in [
        ("graph LR;\nA-->B", Direction::LeftToRight, "LR"),
        ("graph TD\nA-->B", Direction::TopToBottom, "TD"),
        ("graph TB\nA-->B", Direction::TopToBottom, "TB"),
        ("flowchart BT\nA-->B", Direction::BottomToTop, "BT"),
        ("flowchart rl\nA-->B", Direction::RightToLeft, "RL"),
        ("graph\nA-->B", Direction::TopToBottom, "TB"),
        ("flowchart-elk TD\nA-->B", Direction::TopToBottom, "TD"),
    ] {
        let g = g(src);
        assert_eq!((g.direction, g.direction_token.as_str()), (dir, token), "{src:?}");
    }
    // Statements may follow the header on its line.
    let h = g("graph LR; A-->B; B-->C");
    assert_eq!(h.edges.len(), 2);
    assert_eq!(h.direction, Direction::LeftToRight);
}

#[test]
fn detection_is_by_whole_word() {
    assert!(matches!(parse("graphX TD\nA-->B"), Err(IoError::WrongFormat { .. })));
    assert!(matches!(parse("flowchartish\nA-->B"), Err(IoError::WrongFormat { .. })));
    assert!(matches!(parse("sequenceDiagramX\nA->>B: x"), Err(IoError::WrongFormat { .. })));
    assert!(matches!(parse("Graph TD\nA-->B"), Err(IoError::WrongFormat { .. })));
    assert!(matches!(parse("sequenceDiagram\nA->>B: x"), Ok(Diagram::Sequence(_))));
    assert!(matches!(parse("graph TD\nA-->B"), Ok(Diagram::Flowchart(_))));
    // The typed entry points insist on their type.
    assert!(matches!(parse_flowchart("sequenceDiagram\nA->>B: x"), Err(IoError::WrongFormat { .. })));
    assert!(matches!(parse_sequence("graph TD\nA-->B"), Err(IoError::WrongFormat { .. })));
    // The hint names the other type found.
    let Err(IoError::WrongFormat { hint, .. }) = parse("classDiagram\nA <|-- B") else { panic!() };
    assert!(hint.contains("classDiagram"));
}

#[test]
fn class_suffix_on_nodes() {
    let a = g("graph TD\nA:::hot --> B:::cold:::big\nC[text]:::hot");
    assert_eq!(pairs(&a), [("A", "B")]);
    assert_eq!(a.nodes[0].classes, ["hot"]);
    assert_eq!(a.nodes[1].classes, ["cold", "big"]);
    assert_eq!((a.nodes[2].label.as_str(), a.nodes[2].classes.clone()), ("text", vec!["hot".to_string()]));
    let s = g(
        "graph TD\nA:::hot --> B\nclassDef hot fill:#f00,stroke:#00f,stroke-width:3px,color:#fff\nclassDef default fill:#eee",
    );
    let st = s.nodes[0].style.as_ref().unwrap();
    assert_eq!(st.fill, Some(Color::rgb(255, 0, 0)));
    assert_eq!(st.stroke, Some(Color::rgb(0, 0, 255)));
    assert_eq!(st.stroke_width, Some(3.0));
    assert_eq!(st.color, Some(Color::WHITE));
    // The default class applies to nodes without a class.
    assert_eq!(s.nodes[1].style.as_ref().unwrap().fill, Some(Color::rgb(0xee, 0xee, 0xee)));
}

#[test]
fn styles_keep_good_colors_and_drop_bad_ones() {
    let s = g(
        "graph TD\nA\nB\nstyle A fill:#f9f,stroke:notacolor,stroke-width:-2px,color:#12345\nstyle B fill:#abcdef12 !important\nclass A,B big\nstyle Ghost fill:#000",
    );
    let a = s.nodes[0].style.as_ref().unwrap();
    assert_eq!((a.fill, a.stroke, a.stroke_width, a.color), (Some(Color::rgb(255, 0x99, 255)), None, None, None));
    assert_eq!(s.nodes[1].style.as_ref().unwrap().fill, Some(Color::rgba(0xab, 0xcd, 0xef, 0x12)));
    assert_eq!(s.nodes[0].classes, ["big"]);
    assert_eq!(s.nodes.len(), 2, "a style for an unknown id creates no node");
    // Precedence: classDef, then style lines.
    let p = g("graph TD\nA:::c\nclassDef c fill:#111111,stroke:#222222\nstyle A fill:#333333");
    let st = p.nodes[0].style.as_ref().unwrap();
    assert_eq!((st.fill, st.stroke), (Some(Color::rgb(0x33, 0x33, 0x33)), Some(Color::rgb(0x22, 0x22, 0x22))));
}

#[test]
fn directives_that_are_ignored() {
    let s = g(
        "graph TD\nA --> B\nclick A callback \"tip\"\nclick B href \"http://x\" _blank\nlinkStyle 0 stroke:red\nlinkStyle default color:blue\naccTitle: a title\naccDescr: text\naccDescr {\n  multi line\n  description\n}\nB --> C",
    );
    assert_eq!(node_ids(&s), ["A", "B", "C"]);
    assert_eq!(s.edges.len(), 2);
}

#[test]
fn node_named_like_a_directive() {
    let s = g("graph TD\nclass --> style\nstyle --> end2\nx --> click");
    assert_eq!(node_ids(&s), ["class", "style", "end2", "x", "click"]);
    assert_eq!(s.edges.len(), 3);
}

#[test]
fn node_attribute_shape_syntax() {
    let s = g(
        "flowchart TD\nA@{ shape: cyl, label: \"Store\" } --> B@{ shape: diam }\nC@{ shape: nonsense }\nD@{ icon: x }\ne1@{ animate: true }\nA e1@--> B",
    );
    assert_eq!((s.nodes[0].shape, s.nodes[0].label.as_str()), (NodeShape::Cylinder, "Store"));
    assert_eq!((s.nodes[1].shape, s.nodes[1].label.as_str()), (NodeShape::Diamond, "B"));
    assert_eq!(s.nodes[2].shape, NodeShape::Rectangle);
    assert!(!node_ids(&s).contains(&"e1"), "edge attributes are no node");
    assert_eq!(s.edges.len(), 2);
}

#[test]
fn subgraph_title_forms() {
    let s = g(
        "graph TD\nsubgraph My Title\nA\nend\nsubgraph id1 [Title One]\nB\nend\nsubgraph id2[\"Two words\"]\nC\nend\nsubgraph plain\nD\nend\nsubgraph \"Quoted Title\"\nE\nend\nsubgraph My Title\nF\nend",
    );
    let got: Vec<(&str, &str)> = s.subgraphs.iter().map(|x| (x.id.as_str(), x.title.as_str())).collect();
    assert_eq!(
        got,
        [
            ("My_Title", "My Title"),
            ("id1", "Title One"),
            ("id2", "Two words"),
            ("plain", "plain"),
            ("Quoted_Title", "Quoted Title"),
            ("My_Title_2", "My Title"),
        ]
    );
    assert_eq!(s.nodes[0].subgraph, Some(0));
    assert_eq!(s.nodes[5].subgraph, Some(5));
    assert_eq!(s.subgraphs[0].members, ["A"]);
}

#[test]
fn subgraph_structure() {
    let s = g("graph TB\nsubgraph outer\ndirection RL\nA --> B\nsubgraph inner\nC\nend\nB --> C\nD --> A\nend\nE");
    assert_eq!(s.subgraphs[0].direction, Some(Direction::RightToLeft));
    assert_eq!(s.subgraphs[1].parent, Some(0));
    assert_eq!(s.subgraphs[1].direction, None);
    let sg: Vec<Option<usize>> = s.nodes.iter().map(|n| n.subgraph).collect();
    assert_eq!(node_ids(&s), ["A", "B", "C", "D", "E"]);
    assert_eq!(sg, [Some(0), Some(0), Some(1), Some(0), None]);
    // A node first used outside moves into the subgraph that mentions it.
    let m = g("graph TD\nA --> B\nsubgraph s\nA\nend");
    assert_eq!(m.nodes[0].subgraph, Some(0));
    assert_eq!(m.nodes[1].subgraph, None);
    // A subgraph can be referenced before it is declared.
    let late = g("graph TD\nX --> s\nsubgraph s\nA\nend");
    assert_eq!(late.edges[0].to_subgraph, Some(0));
    assert_eq!(node_ids(&late), ["X", "A"]);
    // Unbalanced `end` and unclosed subgraphs do not fail.
    assert!(!g("graph TD\nend\nA-->B").warnings.is_empty());
    assert_eq!(g("graph TD\nsubgraph s\nA-->B").subgraphs.len(), 1);
}

#[test]
fn chained_links_and_all_link_forms() {
    let c = g("graph LR\nA --> B --> C --- D");
    assert_eq!(pairs(&c), [("A", "B"), ("B", "C"), ("C", "D")]);
    assert_eq!(c.edges[2].head_end, Head::None);
    for (src, line, hs, he, label) in [
        ("A-->B", Line::Solid, Head::None, Head::Arrow, None),
        ("A---B", Line::Solid, Head::None, Head::None, None),
        ("A-.->B", Line::Dotted, Head::None, Head::Arrow, None),
        ("A-.-B", Line::Dotted, Head::None, Head::None, None),
        ("A==>B", Line::Thick, Head::None, Head::Arrow, None),
        ("A===B", Line::Thick, Head::None, Head::None, None),
        ("A~~~B", Line::Invisible, Head::None, Head::None, None),
        ("A--xB", Line::Solid, Head::None, Head::Cross, None),
        ("A--oB", Line::Solid, Head::None, Head::Circle, None),
        ("A<-->B", Line::Solid, Head::Arrow, Head::Arrow, None),
        ("A x--x B", Line::Solid, Head::Cross, Head::Cross, None),
        ("A o--o B", Line::Solid, Head::Circle, Head::Circle, None),
        ("A <-- B", Line::Solid, Head::Arrow, Head::None, None),
        ("A <== B", Line::Thick, Head::Arrow, Head::None, None),
        ("A<==>B", Line::Thick, Head::Arrow, Head::Arrow, None),
        ("A ----> B", Line::Solid, Head::None, Head::Arrow, None),
        ("A =====> B", Line::Thick, Head::None, Head::Arrow, None),
        ("A -....-> B", Line::Dotted, Head::None, Head::Arrow, None),
        ("A -->|text| B", Line::Solid, Head::None, Head::Arrow, Some("text")),
        ("A -- text --> B", Line::Solid, Head::None, Head::Arrow, Some("text")),
        ("A -- some text --- B", Line::Solid, Head::None, Head::None, Some("some text")),
        ("A -. text .-> B", Line::Dotted, Head::None, Head::Arrow, Some("text")),
        ("A == text ==> B", Line::Thick, Head::None, Head::Arrow, Some("text")),
        ("A ==>|text| B", Line::Thick, Head::None, Head::Arrow, Some("text")),
        ("A -->| spaced | B", Line::Solid, Head::None, Head::Arrow, Some("spaced")),
        ("A -- \"quoted\" --> B", Line::Solid, Head::None, Head::Arrow, Some("quoted")),
        ("A -->|\"q | p\"| B", Line::Solid, Head::None, Head::Arrow, Some("q | p")),
    ] {
        let gr = g(&format!("graph TD\n{src}"));
        assert_eq!(gr.edges.len(), 1, "{src}");
        let e = &gr.edges[0];
        assert_eq!((e.from.as_str(), e.to.as_str()), ("A", "B"), "{src}");
        assert_eq!((e.line, e.head_start, e.head_end, e.label.as_deref()), (line, hs, he, label), "{src}");
    }
    // `o` and `x` right after a space start a node.
    let n = g("graph TD\nA --- oB\nA --- xB");
    assert_eq!(node_ids(&n), ["A", "oB", "xB"]);
    assert_eq!(n.edges[0].head_end, Head::None);
}

#[test]
fn thick_stays_thick_and_direction_is_kept() {
    let t = g("graph TD\nA ==> B\nB <== C");
    assert_eq!(t.edges[0].line, Line::Thick);
    assert_eq!((t.edges[1].line, t.edges[1].head_start, t.edges[1].head_end), (Line::Thick, Head::Arrow, Head::None));
    assert_eq!((t.edges[1].from.as_str(), t.edges[1].to.as_str()), ("B", "C"));
}

#[test]
fn all_node_shapes() {
    for (src, shape, label) in [
        ("A[x y]", NodeShape::Rectangle, "x y"),
        ("A(x)", NodeShape::Rounded, "x"),
        ("A([x])", NodeShape::Stadium, "x"),
        ("A[[x]]", NodeShape::Subroutine, "x"),
        ("A((x))", NodeShape::Circle, "x"),
        ("A(((x)))", NodeShape::DoubleCircle, "x"),
        ("A{x}", NodeShape::Diamond, "x"),
        ("A{{x}}", NodeShape::Hexagon, "x"),
        ("A[(db)]", NodeShape::Cylinder, "db"),
        ("A[/x/]", NodeShape::Parallelogram, "x"),
        ("A[\\x\\]", NodeShape::ParallelogramAlt, "x"),
        ("A[/x\\]", NodeShape::Trapezoid, "x"),
        ("A[\\x/]", NodeShape::TrapezoidAlt, "x"),
        ("A>x]", NodeShape::Asymmetric, "x"),
        ("A", NodeShape::Rectangle, "A"),
        ("A[]", NodeShape::Rectangle, "A"),
        ("A[\"with (parens) and {braces}\"]", NodeShape::Rectangle, "with (parens) and {braces}"),
    ] {
        let gr = g(&format!("graph TD\n{src} --> Z"));
        assert_eq!((gr.nodes[0].shape, gr.nodes[0].label.as_str()), (shape, label), "{src}");
        assert_eq!(gr.edges.len(), 1, "{src}");
    }
}

#[test]
fn label_cleaning() {
    let l = |s: &str| g(&format!("graph TD\nA[\"{s}\"]")).nodes[0].label.clone();
    assert_eq!(l("a<br>b<br/>c<br />d<BR>e"), "a\nb\nc\nd\ne");
    assert_eq!(l("say #quot;hi#quot; #35; #amp; #nosuch;"), "say \"hi\" # & #nosuch;");
    assert_eq!(l("&lt;b&gt; &amp; &#65;&#x42;"), "<b> & AB");
    assert_eq!(l("`markdown **bold**`"), "markdown **bold**");
    assert_eq!(l("  padded  "), "padded");
    assert_eq!(l("日本語 ✓"), "日本語 ✓");
    // A quote inside a bracket label that starts without a quote is plain text.
    assert_eq!(g("graph TD\nA[say \"hi\"]").nodes[0].label, "say \"hi\"");
    // Pipe labels are cleaned the same way.
    assert_eq!(g("graph TD\nA -->|a<br>b| B").edges[0].label.as_deref(), Some("a\nb"));
}

#[test]
fn redefinition_takes_the_last_explicit_shape_and_label() {
    let r = g("graph TD\nA[one] --> B\nA((two)) --> C\nA --> D");
    assert_eq!((r.nodes[0].shape, r.nodes[0].label.as_str()), (NodeShape::Circle, "two"));
    assert_eq!(r.edges.len(), 3);
}

#[test]
fn fence_and_front_matter() {
    let f = g("---\ntitle: Hello\nconfig:\n  theme: dark\n---\ngraph LR\nA-->B");
    assert_eq!(node_ids(&f), ["A", "B"]);
    let fenced = g("```mermaid\n---\ntitle: x\n---\ngraph TD\nA-->B\n```\n");
    assert_eq!(node_ids(&fenced), ["A", "B"]);
    let tilde = g("~~~\ngraph TD\nA-->B\n~~~");
    assert_eq!(tilde.edges.len(), 1);
    let crlf = g("graph TD\r\nA --> B\r\nB --> C\r\n");
    assert_eq!(crlf.edges.len(), 2);
    let bom = g("\u{feff}graph TD\nA-->B");
    assert_eq!(bom.edges.len(), 1);
    assert!(matches!(parse("---\ntitle: never closed\ngraph TD\nA-->B"), Err(IoError::Parse { .. })));
    // A sequence diagram in a fence.
    assert!(matches!(parse("```mermaid\nsequenceDiagram\nA->>B: hi\n```"), Ok(Diagram::Sequence(_))));
}

#[test]
fn bad_statements_are_skipped_not_fatal() {
    let s = g("graph TD\nA --> B\nA --> \nthis is %%$ nonsense (\nB --> C\n--> D\nE[unclosed --> F");
    assert_eq!(pairs(&s), [("A", "B"), ("B", "C")]);
    assert!(!s.warnings.is_empty());
}

#[test]
fn flowchart_serde_round_trip() {
    let src = read("mermaid/flowchart_subgraph.mmd") + "\nstyle A fill:#f9f,stroke-width:2.5px";
    let graph = g(&src);
    let json = serde_json::to_string(&graph).unwrap();
    assert_eq!(serde_json::from_str::<MermaidGraph>(&json).unwrap(), graph);
    let d = parse(&read("mermaid/sequence_notes_self.mmd")).unwrap();
    let json = serde_json::to_string(&d).unwrap();
    assert_eq!(serde_json::from_str::<Diagram>(&json).unwrap(), d);
}

// ---------------------------------------------------------------------------------------------------------------
// Sequence diagrams

#[test]
fn block_keywords_never_become_messages() {
    let d = seq(
        "sequenceDiagram\n  A->>B: one\n  alt x -> y\n    B->>A: inside alt\n  else a --> b\n    A->>B: inside else\n  end\n  opt maybe ->> yes\n    A-xB: opt\n  end\n  loop forever -) here\n    B--)A: loop\n  end\n  par a -> b\n    A->B: par\n  and c -> d\n    B->A: and\n  end\n  critical a -> b\n  option c -> d\n  break e -> f\n  rect rgb(0,0,0)\n  end\n  box Group\n  end\n  title: A -> B\n  title Another -> One\n  autonumber\n  activate A\n  deactivate B\n  destroy B\n  link A: Dashboard @ https://x\n  A->>B: last",
    );
    let texts: Vec<&str> = d
        .events
        .iter()
        .map(|e| match e {
            Event::Message { text, .. } => text.as_str(),
            Event::Note { .. } => "note",
        })
        .collect();
    assert_eq!(texts, ["one", "inside alt", "inside else", "opt", "loop", "par", "and", "last"]);
    let ids: Vec<&str> = d.participants.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["A", "B"], "no participant came from a block line");
}

#[test]
fn activation_markers_and_text_with_arrows() {
    let d = seq("sequenceDiagram\nA->>+B: call\nB-->>-A: reply -> done\nA->>B: x: y");
    assert_eq!(d.participants.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["A", "B"]);
    let Event::Message { text, activation, to, .. } = &d.events[0] else { panic!() };
    assert_eq!((text.as_str(), *activation, to.as_str()), ("call", Activation::Activate, "B"));
    let Event::Message { text, activation, from, to, dashed, .. } = &d.events[1] else { panic!() };
    assert_eq!(
        (text.as_str(), *activation, from.as_str(), to.as_str(), *dashed),
        ("reply -> done", Activation::Deactivate, "B", "A", true)
    );
    let Event::Message { text, .. } = &d.events[2] else { panic!() };
    assert_eq!(text, "x: y");
}

#[test]
fn participant_declarations() {
    let d = seq(
        "sequenceDiagram\nparticipant A as Alice\nactor B as Bob the <br>Builder\nparticipant C@{ \"type\": \"database\" }\nparticipant D\nparticipant D as Dee\ncreate participant E as Eve\ncreate actor F\nA->>B: hi\nactor A",
    );
    let got: Vec<(&str, &str, bool)> =
        d.participants.iter().map(|p| (p.id.as_str(), p.label.as_str(), p.is_actor)).collect();
    assert_eq!(
        got,
        [
            ("A", "Alice", true),
            ("B", "Bob the \nBuilder", true),
            ("C", "C", false),
            ("D", "Dee", false),
            ("E", "Eve", false),
            ("F", "F", true),
        ]
    );
    // `alias` contains `as` but is no keyword.
    let a = seq("sequenceDiagram\nparticipant alias");
    assert_eq!(a.participants[0].id, "alias");
}

#[test]
fn notes_create_participants_and_keep_order() {
    let d = seq("sequenceDiagram\nNote over X,Y: both\nnote left of Z: l\nNote right of X\nA->>B: m");
    assert_eq!(d.participants.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["X", "Y", "Z", "A", "B"]);
    assert_eq!(d.events.len(), 4);
    assert!(matches!(&d.events[2], Event::Note { placement: NotePlacement::RightOf, text, .. } if text.is_empty()));
}

#[test]
fn sequence_garbage_lines_are_skipped() {
    let d =
        seq("sequenceDiagram\nthis is not a message\nA->>B: ok\n->>B: no sender\nA->>: no receiver\nNote nowhere: x");
    assert_eq!(d.events.len(), 1);
    assert!(d.warnings.len() >= 3);
}

// ---------------------------------------------------------------------------------------------------------------
// Limits and hostile input

#[test]
fn cross_product_bomb_is_refused_before_allocating() {
    let side = 5000;
    let a: Vec<String> = (0..side).map(|i| format!("A{i}")).collect();
    let b: Vec<String> = (0..side).map(|i| format!("B{i}")).collect();
    let src = format!("graph TD\n{} --> {}", a.join(" & "), b.join(" & "));
    let t = Instant::now();
    let err = parse(&src).unwrap_err();
    assert!(matches!(err, IoError::Limit { .. }), "{err:?}");
    assert!(t.elapsed().as_secs() < 10);
    // A chain of products is checked the same way.
    let c: Vec<String> = (0..300).map(|i| format!("C{i}")).collect();
    let src = format!("graph TD\n{0} --> {0} --> {0}", c.join(" & "));
    assert!(matches!(parse(&src), Err(IoError::Limit { .. })));
    // Too many statements are refused too.
    let many: String = (0..MAX_DIAGRAM_ITEMS).map(|i| format!("N{i}\n")).collect();
    assert!(parse(&format!("graph TD\n{many}")).is_ok());
    assert!(matches!(parse(&format!("graph TD\n{many}Extra")), Err(IoError::Limit { .. })));
    // The same for sequence diagrams.
    let msgs: String = (0..MAX_DIAGRAM_ITEMS + 5).map(|i| format!("A->>B{i}: m\n")).collect();
    assert!(matches!(parse(&format!("sequenceDiagram\n{msgs}")), Err(IoError::Limit { .. })));
}

#[test]
fn a_label_on_a_product_cannot_blow_up_memory() {
    let label = "x".repeat(MAX_LABEL_CHARS);
    let a: Vec<String> = (0..95).map(|i| format!("A{i}")).collect();
    let b: Vec<String> = (0..95).map(|i| format!("B{i}")).collect();
    let src = format!("graph TD\n{} -->|{label}| {}", a.join("&"), b.join("&"));
    // 9025 edges of 10k chars would be 90 MB of text.
    assert!(matches!(parse(&src), Err(IoError::Limit { .. })));
}

#[test]
fn too_many_lines_and_long_text() {
    let lines = "A-->B\n".repeat(MAX_DIAGRAM_LINES + 1);
    assert!(matches!(parse(&format!("graph TD\n{lines}")), Err(IoError::Limit { .. })));
    let long = "é".repeat(MAX_LABEL_CHARS * 2);
    let gr = g(&format!("graph TD\nA[{long}]"));
    assert_eq!(gr.nodes[0].label.chars().count(), MAX_LABEL_CHARS);
    let s = seq(&format!("sequenceDiagram\nA->>B: {long}"));
    let Event::Message { text, .. } = &s.events[0] else { panic!() };
    assert_eq!(text.chars().count(), MAX_LABEL_CHARS);
    let id = "a".repeat(5000);
    assert!(matches!(parse(&format!("graph TD\n{id}-->B")), Err(IoError::Limit { .. })));
    assert!(matches!(parse(&format!("sequenceDiagram\n{id}->>B: x")), Err(IoError::Limit { .. })));
}

#[test]
fn deep_subgraph_nesting() {
    let nest = |depth: usize| {
        let mut s = String::from("graph TD\n");
        for i in 0..depth {
            s.push_str(&format!("subgraph s{i}\n"));
        }
        s.push_str("A\n");
        for _ in 0..depth {
            s.push_str("end\n");
        }
        s
    };
    let ok = g(&nest(MAX_SUBGRAPH_DEPTH));
    assert_eq!(ok.subgraphs.len(), MAX_SUBGRAPH_DEPTH);
    assert_eq!(ok.nodes[0].subgraph, Some(MAX_SUBGRAPH_DEPTH - 1));
    assert!(matches!(parse(&nest(MAX_SUBGRAPH_DEPTH + 1)), Err(IoError::Limit { .. })));
    // Far deeper than any stack could take recursively.
    assert!(matches!(parse(&nest(100_000 / 2 - 2)), Err(IoError::Limit { .. })));
}

#[test]
fn large_inputs_parse_fast() {
    let mut src = String::from("graph LR\n");
    for i in 0..20_000 {
        src.push_str(&format!("N{i}[Node {i}] -->|e{i}| N{}\n", i + 1));
        if i == 4_000 {
            src.push_str("subgraph s\nN1\nend\n");
        }
    }
    let t = Instant::now();
    // 20k statements make 20k edges: over the item limit, which must be refused quickly.
    assert!(matches!(parse(&src), Err(IoError::Limit { .. })));
    // Within the limits (4_999 nodes plus 4_998 edges) it must finish fast.
    let mut ok = String::from("graph LR\n");
    for i in 0..4_998 {
        ok.push_str(&format!("N{i}[Node {i}] -->|e{i}| N{}\n", i + 1));
    }
    let gr = g(&ok);
    assert_eq!(gr.edges.len(), 4_998);
    let mut sq = String::from("sequenceDiagram\n");
    for i in 0..9_000 {
        sq.push_str(&format!("A->>B: message {i}\n"));
    }
    assert_eq!(seq(&sq).events.len(), 9_000);
    // Many comment and blank lines are cheap.
    let noise = "%% c\n\n   \n".repeat(30_000);
    assert!(parse(&format!("graph TD\n{noise}A-->B")).is_ok());
    assert!(t.elapsed().as_secs() < 20, "took {:?}", t.elapsed());
}

#[test]
fn nasty_inputs_do_not_panic() {
    for src in [
        "",
        "\n\n\n",
        "graph",
        "graph TD\n[",
        "graph TD\nA[",
        "graph TD\nA[[",
        "graph TD\nA((x)",
        "graph TD\nA-->",
        "graph TD\n-->",
        "graph TD\n&",
        "graph TD\nA & --> B",
        "graph TD\nA -- --> B",
        "graph TD\nA -- \"unclosed --> B",
        "graph TD\nA -->|unclosed B",
        "graph TD\nA@{",
        "graph TD\nA@{ shape: }",
        "graph TD\nA:::",
        "graph TD\nsubgraph\nend",
        "graph TD\nsubgraph [\nend",
        "graph TD\nsubgraph ]]\nA\nend",
        "graph TD\nclassDef",
        "graph TD\nclassDef a",
        "graph TD\nstyle",
        "graph TD\nstyle A",
        "graph TD\nstyle A fill",
        "graph TD\nclass",
        "graph TD\ndirection",
        "graph TD\nA[é]-->B[日本]\u{301}",
        "graph TD\nA[\u{0}]",
        "graph TD\nA[#99999999999;]",
        "graph TD\nA[&#x110000;]",
        "graph TD\nA[&#xD800;]",
        "graph TD\nA[#;]",
        "sequenceDiagram",
        "sequenceDiagram\nNote",
        "sequenceDiagram\nNote over",
        "sequenceDiagram\nNote over :",
        "sequenceDiagram\n->>",
        "sequenceDiagram\nA->>",
        "sequenceDiagram\nA->>+",
        "sequenceDiagram\nA->>+:",
        "sequenceDiagram\nparticipant",
        "sequenceDiagram\nparticipant as",
        "sequenceDiagram\nparticipant  as  x",
        "sequenceDiagram\nparticipant X as",
        "sequenceDiagram\ncreate",
        "sequenceDiagram\nÀ->>é: ü",
        "---",
        "```",
        "```mermaid",
        "```mermaid\n```",
        "%%{init:",
        "\u{feff}",
        "graph TD\nA --> B\u{2028}C",
    ] {
        let _ = parse(src);
        let _ = parse_flowchart(src);
        let _ = parse_sequence(src);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Property tests

/// What every successful flowchart satisfies, whatever the input.
fn check_graph(gr: &MermaidGraph) -> Result<(), TestCaseError> {
    prop_assert!(gr.nodes.len() + gr.edges.len() + gr.subgraphs.len() <= MAX_DIAGRAM_ITEMS);
    let ids: BTreeMap<&str, usize> = gr.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    prop_assert_eq!(ids.len(), gr.nodes.len(), "node ids are unique");
    for n in &gr.nodes {
        prop_assert!(n.label.chars().count() <= MAX_LABEL_CHARS);
        if let Some(s) = n.subgraph {
            prop_assert!(s < gr.subgraphs.len());
            prop_assert!(gr.subgraphs[s].members.contains(&n.id));
        }
    }
    for e in &gr.edges {
        prop_assert!(e.label.as_ref().is_none_or(|l| l.chars().count() <= MAX_LABEL_CHARS));
        match e.from_subgraph {
            Some(s) => prop_assert!(s < gr.subgraphs.len() && gr.subgraphs[s].id == e.from),
            None => prop_assert!(ids.contains_key(e.from.as_str()), "edge from unknown node {:?}", e.from),
        }
        match e.to_subgraph {
            Some(s) => prop_assert!(s < gr.subgraphs.len() && gr.subgraphs[s].id == e.to),
            None => prop_assert!(ids.contains_key(e.to.as_str()), "edge to unknown node {:?}", e.to),
        }
    }
    for (i, s) in gr.subgraphs.iter().enumerate() {
        prop_assert!(s.parent.is_none_or(|p| p < i));
        let mut depth = 1;
        let mut cur = s.parent;
        while let Some(p) = cur {
            depth += 1;
            cur = gr.subgraphs[p].parent;
        }
        prop_assert!(depth <= MAX_SUBGRAPH_DEPTH);
        for m in &s.members {
            prop_assert!(ids.contains_key(m.as_str()));
        }
    }
    Ok(())
}

fn check_seq(d: &SequenceDiagram) -> Result<(), TestCaseError> {
    prop_assert!(d.participants.len() + d.events.len() <= MAX_DIAGRAM_ITEMS);
    let ids: Vec<&str> = d.participants.iter().map(|p| p.id.as_str()).collect();
    for e in &d.events {
        match e {
            Event::Message { from, to, text, .. } => {
                prop_assert!(ids.contains(&from.as_str()) && ids.contains(&to.as_str()));
                prop_assert!(text.chars().count() <= MAX_LABEL_CHARS);
            }
            Event::Note { participants, text, .. } => {
                prop_assert!(!participants.is_empty() && participants.iter().all(|p| ids.contains(&p.as_str())));
                prop_assert!(text.chars().count() <= MAX_LABEL_CHARS);
            }
        }
    }
    Ok(())
}

fn check_any(src: &str) -> Result<(), TestCaseError> {
    match parse(src) {
        Ok(Diagram::Flowchart(gr)) => check_graph(&gr),
        Ok(Diagram::Sequence(d)) => check_seq(&d),
        Err(_) => Ok(()),
    }
}

const FLOW_TOKENS: &[&str] = &[
    "A",
    "B",
    "C1",
    "x",
    "o",
    "end",
    "subgraph",
    "direction",
    "LR",
    "TD",
    " ",
    " ",
    " ",
    "\n",
    "\n",
    ";",
    "&",
    "-->",
    "---",
    "-.->",
    "-.-",
    "==>",
    "===",
    "~~~",
    "--x",
    "--o",
    "<-->",
    "<--",
    "--",
    "-",
    "->",
    ".",
    "=",
    "|",
    "|text|",
    "[",
    "]",
    "(",
    ")",
    "{",
    "}",
    "[[",
    "]]",
    "((",
    "))",
    "(((",
    ")))",
    "[(",
    ")]",
    "([",
    "])",
    "[/",
    "/]",
    "[\\",
    "\\]",
    ">",
    "\"",
    "`",
    ":::",
    "@{",
    "}",
    "shape:",
    "cyl",
    ",",
    "style",
    "classDef",
    "class",
    "click",
    "linkStyle",
    "fill:#f9f",
    "stroke:#333",
    "%%",
    "<br>",
    "#quot;",
    "&amp;",
    "é",
    "日本",
    "😀",
    "e1@",
    "'",
    "\\",
    "/",
    "#",
    ":",
    "!",
];

const SEQ_TOKENS: &[&str] = &[
    "participant",
    "actor",
    "as",
    "Note",
    "note",
    "over",
    "left",
    "right",
    "of",
    "A",
    "B",
    "Ü",
    ",",
    ":",
    "->>",
    "-->>",
    "->",
    "-->",
    "-x",
    "--x",
    "-)",
    "--)",
    "<<->>",
    "<<-->>",
    "+",
    "-",
    "\n",
    "\n",
    "\n",
    " ",
    " ",
    "alt",
    "else",
    "end",
    "loop",
    "activate",
    "title",
    "autonumber",
    "%%",
    "<br/>",
    "@{",
    "}",
    "create",
];

fn soup(tokens: &'static [&'static str], max: usize) -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(tokens), 0..max).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_text_never_panics(s in any::<String>()) {
        check_any(&s)?;
        check_any(&format!("graph TD\n{s}"))?;
        check_any(&format!("sequenceDiagram\n{s}"))?;
    }

    #[test]
    fn arbitrary_text_with_diagram_alphabet_never_panics(s in "[A-Za-z0-9 \\-\\.=~<>|\\[\\](){}\"`;:&@#%/\\\\\n\t,+xo]{0,300}") {
        check_any(&format!("graph LR\n{s}"))?;
        check_any(&format!("sequenceDiagram\n{s}"))?;
    }

    #[test]
    fn flowchart_token_soup_respects_the_caps(s in soup(FLOW_TOKENS, 120)) {
        check_any(&format!("flowchart TD\n{s}"))?;
        check_any(&s)?;
    }

    #[test]
    fn sequence_token_soup_respects_the_caps(s in soup(SEQ_TOKENS, 120)) {
        check_any(&format!("sequenceDiagram\n{s}"))?;
    }

    #[test]
    fn serde_round_trip_of_whatever_parses(s in soup(FLOW_TOKENS, 80)) {
        if let Ok(d) = parse(&format!("graph TD\n{s}")) {
            let json = serde_json::to_string(&d).unwrap();
            prop_assert_eq!(serde_json::from_str::<Diagram>(&json).unwrap(), d);
        }
    }

    #[test]
    fn product_sizes_never_exceed_the_cap(a in 1usize..200, b in 1usize..200, label in "[a-z ]{0,20}") {
        let left: Vec<String> = (0..a).map(|i| format!("L{i}")).collect();
        let right: Vec<String> = (0..b).map(|i| format!("R{i}")).collect();
        let src = format!("graph TD\n{} -->|{label}| {}", left.join(" & "), right.join(" & "));
        match parse_flowchart(&src) {
            Ok(gr) => {
                prop_assert_eq!(gr.edges.len(), a * b);
                prop_assert_eq!(gr.nodes.len(), a + b);
            }
            Err(e) => {
                let is_limit = matches!(e, IoError::Limit { .. });
                prop_assert!(is_limit);
                prop_assert!(a * b + a + b > MAX_DIAGRAM_ITEMS);
            }
        }
    }
}
