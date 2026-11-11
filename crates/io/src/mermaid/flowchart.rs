//! Flowchart parser. One pass over the lines with an explicit subgraph stack; styles and subgraph endpoints are
//! resolved after the pass so that they may appear in any order.

use std::collections::{HashMap, HashSet};

use drafftink_scene::Color;

use super::text::{Cur, clean_label, find_closer, is_id_char, shorten, split_statements, split_word};
use super::types::{Direction, Edge, Head, Line, MermaidGraph, Node, NodeShape, NodeStyle, Subgraph};
use super::{MAX_ID_CHARS, MAX_SUBGRAPH_DEPTH, MAX_TOTAL_TEXT_BYTES, Prepared, push_warning};
use crate::error::{FormatName, IoError};
use crate::limits::MAX_DIAGRAM_ITEMS;

/// Most class names kept on one node.
const MAX_CLASSES_PER_NODE: usize = 64;
/// Largest stroke width kept.
const MAX_STROKE_WIDTH: f64 = 1_000.0;

pub(super) fn parse_direction(token: &str) -> Option<Direction> {
    match token.trim().to_ascii_uppercase().as_str() {
        "TB" | "TD" => Some(Direction::TopToBottom),
        "BT" => Some(Direction::BottomToTop),
        "LR" => Some(Direction::LeftToRight),
        "RL" => Some(Direction::RightToLeft),
        _ => None,
    }
}

pub(super) fn parse(p: &Prepared<'_>) -> Result<MermaidGraph, IoError> {
    let mut b = Builder { token: "TB".to_string(), ..Builder::default() };

    // The header: `graph TD`, `flowchart LR;`, or `graph TD; A-->B`.
    let mut head_statements = split_statements(p.header_rest).into_iter();
    if let Some(first) = head_statements.next() {
        let first = first.trim();
        if !first.is_empty() {
            let (word, extra) = split_word(first);
            if let Some(d) = parse_direction(word) {
                b.direction = d;
                b.token = word.to_ascii_uppercase();
                if !extra.is_empty() {
                    b.warn(format!("ignored text after the direction: {}", shorten(extra, 40)));
                }
            } else {
                b.warn(format!("unknown direction {:?}; using top to bottom", shorten(word, 20)));
            }
        }
    }
    for st in head_statements {
        b.line_statement(st)?;
    }

    for line in &p.lines {
        if b.in_acc_block {
            if line.contains('}') {
                b.in_acc_block = false;
            }
            continue;
        }
        for st in split_statements(line) {
            b.line_statement(st)?;
        }
    }
    b.finish()
}

#[derive(Default)]
struct Builder {
    direction: Direction,
    token: String,
    nodes: Vec<Node>,
    /// Parallel to `nodes`: true while the node has only been named, never given a label, shape or class.
    bare: Vec<bool>,
    index: HashMap<String, usize>,
    edges: Vec<Edge>,
    subgraphs: Vec<Subgraph>,
    /// Parallel to `subgraphs`: the id was written in the text (and edges may refer to it).
    explicit_id: Vec<bool>,
    used_subgraph_ids: HashSet<String>,
    suffix_counter: HashMap<String, usize>,
    /// Open subgraphs, innermost last.
    stack: Vec<usize>,
    class_defs: HashMap<String, NodeStyle>,
    class_assign: Vec<(String, String)>,
    style_lines: Vec<(String, NodeStyle)>,
    text_bytes: usize,
    in_acc_block: bool,
    warnings: Vec<String>,
}

/// A syntax problem in one statement (skipped, reported) or a hard failure (the whole import stops).
enum PErr {
    Syntax(String),
    Fatal(IoError),
}

impl From<IoError> for PErr {
    fn from(e: IoError) -> Self {
        PErr::Fatal(e)
    }
}

fn limit_items() -> IoError {
    IoError::Limit { what: "diagram (nodes, edges and subgraphs)", max: format!("{MAX_DIAGRAM_ITEMS} items") }
}

fn limit_id() -> IoError {
    IoError::Limit { what: "identifier", max: format!("{MAX_ID_CHARS} characters") }
}

fn limit_text() -> IoError {
    IoError::Limit { what: "text of the diagram (ids and labels)", max: format!("{MAX_TOTAL_TEXT_BYTES} bytes") }
}

#[derive(Debug, Clone)]
struct NodeDecl {
    id: String,
    label: Option<String>,
    shape: Option<NodeShape>,
    classes: Vec<String>,
    /// Only the ids and attributes of a `e1@{ animate: true }` statement: not a node.
    edge_attrs: bool,
    /// `@{ shape: ... }` named a shape that is not known.
    unknown_shape: bool,
}

#[derive(Debug, Clone)]
struct LinkSpec {
    line: Line,
    head_start: Head,
    head_end: Head,
    label: Option<String>,
}

struct Chain {
    groups: Vec<Vec<NodeDecl>>,
    links: Vec<LinkSpec>,
}

impl Builder {
    fn warn(&mut self, message: String) {
        push_warning(&mut self.warnings, message);
    }

    fn items(&self) -> usize {
        self.nodes.len() + self.edges.len() + self.subgraphs.len()
    }

    fn add_text(&mut self, bytes: usize) -> Result<(), IoError> {
        self.text_bytes = self.text_bytes.saturating_add(bytes);
        if self.text_bytes > MAX_TOTAL_TEXT_BYTES { Err(limit_text()) } else { Ok(()) }
    }

    /// One trimmed, non-empty statement (or an empty one, which is skipped).
    fn line_statement(&mut self, raw: &str) -> Result<(), IoError> {
        let st = raw.trim();
        if st.is_empty() {
            return Ok(());
        }
        self.statement(st)
    }

    fn statement(&mut self, st: &str) -> Result<(), IoError> {
        let (word, rest) = split_word(st);
        match word {
            "subgraph" => return self.open_subgraph(rest),
            "end" if rest.is_empty() => {
                if self.stack.pop().is_none() {
                    self.warn("an `end` without a matching `subgraph` was ignored".to_string());
                }
                return Ok(());
            }
            "direction" => {
                if let Some(d) = parse_direction(rest) {
                    if let Some(&top) = self.stack.last()
                        && let Some(sg) = self.subgraphs.get_mut(top)
                    {
                        sg.direction = Some(d);
                    }
                    return Ok(());
                }
            }
            "classDef" | "class" | "style" | "click" | "linkStyle" if !rest.is_empty() && !starts_like_link(rest) => {
                return self.directive(word, rest);
            }
            _ => {}
        }
        if let Some(tail) = st.strip_prefix("accDescr")
            && (tail.trim_start().starts_with(':') || tail.trim_start().starts_with('{'))
        {
            if tail.trim_start().starts_with('{') && !tail.contains('}') {
                self.in_acc_block = true;
            }
            return Ok(());
        }
        if let Some(tail) = st.strip_prefix("accTitle")
            && (tail.trim_start().starts_with(':') || tail.chars().next().is_some_and(char::is_whitespace))
        {
            return Ok(());
        }
        match parse_chain(st) {
            Ok(chain) => self.commit(chain),
            Err(PErr::Syntax(msg)) => {
                self.warn(format!("skipped a statement ({msg}): {}", shorten(st, 60)));
                Ok(())
            }
            Err(PErr::Fatal(e)) => Err(e),
        }
    }

    fn directive(&mut self, word: &str, rest: &str) -> Result<(), IoError> {
        match word {
            "classDef" => {
                let (names, props) = split_word(rest);
                let style = parse_props(props);
                for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                    self.add_text(name.len())?;
                    self.class_defs.entry(name.to_string()).or_default().merge(&style);
                }
            }
            "class" => {
                let (ids, tail) = split_word(rest);
                let (name, _) = split_word(tail);
                if name.is_empty() {
                    return Ok(());
                }
                for id in ids.split(',').map(str::trim).filter(|i| !i.is_empty()) {
                    if self.class_assign.len() >= MAX_DIAGRAM_ITEMS {
                        self.warn("too many `class` assignments; the rest was ignored".to_string());
                        break;
                    }
                    self.class_assign.push((id.to_string(), name.to_string()));
                }
            }
            "style" => {
                let (id, props) = split_word(rest);
                let style = parse_props(props);
                if !style.is_empty() {
                    if self.style_lines.len() >= MAX_DIAGRAM_ITEMS {
                        self.warn("too many `style` lines; the rest was ignored".to_string());
                    } else {
                        self.style_lines.push((id.to_string(), style));
                    }
                }
            }
            // `click` and `linkStyle` are accepted and ignored.
            _ => {}
        }
        Ok(())
    }

    fn open_subgraph(&mut self, rest: &str) -> Result<(), IoError> {
        if self.stack.len() >= MAX_SUBGRAPH_DEPTH {
            return Err(IoError::Limit { what: "subgraph nesting", max: format!("{MAX_SUBGRAPH_DEPTH} levels") });
        }
        if self.items() >= MAX_DIAGRAM_ITEMS {
            return Err(limit_items());
        }
        let (explicit, title) = parse_subgraph_head(rest)?;
        let is_explicit = explicit.is_some();
        let mut base = match explicit {
            Some(id) => id,
            None => derive_id(&title),
        };
        if title.chars().count() > super::MAX_LABEL_CHARS {
            base = derive_id(&base);
        }
        let id = self.unique_subgraph_id(base, is_explicit);
        self.add_text(id.len() + title.len())?;
        self.used_subgraph_ids.insert(id.clone());
        self.subgraphs.push(Subgraph {
            id,
            title,
            parent: self.stack.last().copied(),
            direction: None,
            members: Vec::new(),
        });
        self.explicit_id.push(is_explicit);
        self.stack.push(self.subgraphs.len() - 1);
        Ok(())
    }

    fn unique_subgraph_id(&mut self, base: String, explicit: bool) -> String {
        if !self.used_subgraph_ids.contains(&base) {
            return base;
        }
        if explicit {
            self.warn(format!("the subgraph id {:?} is used twice; the second one was renamed", shorten(&base, 30)));
        }
        let counter = self.suffix_counter.entry(base.clone()).or_insert(1);
        loop {
            *counter += 1;
            let candidate = format!("{base}_{counter}");
            if !self.used_subgraph_ids.contains(&candidate) {
                return candidate;
            }
        }
    }

    fn is_ancestor(&self, ancestor: usize, of: usize) -> bool {
        let mut cur = self.subgraphs.get(of).and_then(|s| s.parent);
        let mut steps = 0;
        while let Some(i) = cur {
            if i == ancestor {
                return true;
            }
            steps += 1;
            if steps > MAX_SUBGRAPH_DEPTH + 1 {
                return false;
            }
            cur = self.subgraphs.get(i).and_then(|s| s.parent);
        }
        false
    }

    /// Creates or updates the node and returns its index. `None` for pure edge attributes.
    fn touch(&mut self, d: &NodeDecl) -> Result<Option<usize>, IoError> {
        if d.unknown_shape {
            self.warn(format!("unknown shape for node {:?}; drawn as a rectangle", shorten(&d.id, 30)));
        }
        let existing = self.index.get(&d.id).copied();
        if d.edge_attrs && existing.is_none() {
            return Ok(None);
        }
        let idx = match existing {
            Some(i) => i,
            None => {
                if self.items() >= MAX_DIAGRAM_ITEMS {
                    return Err(limit_items());
                }
                self.add_text(d.id.len() * 2)?;
                self.nodes.push(Node {
                    id: d.id.clone(),
                    label: d.id.clone(),
                    shape: NodeShape::Rectangle,
                    subgraph: None,
                    classes: Vec::new(),
                    style: None,
                });
                self.bare.push(true);
                let i = self.nodes.len() - 1;
                self.index.insert(d.id.clone(), i);
                i
            }
        };
        let current = self.stack.last().copied();
        let mut label_bytes = 0;
        if let Some(node) = self.nodes.get_mut(idx) {
            if let Some(label) = &d.label
                && !label.is_empty()
            {
                label_bytes = label.len();
                node.label = label.clone();
                if let Some(b) = self.bare.get_mut(idx) {
                    *b = false;
                }
            }
            if let Some(shape) = d.shape {
                node.shape = shape;
                if let Some(b) = self.bare.get_mut(idx) {
                    *b = false;
                }
            }
            for c in &d.classes {
                if node.classes.len() < MAX_CLASSES_PER_NODE && !node.classes.contains(c) {
                    label_bytes += c.len();
                    node.classes.push(c.clone());
                }
            }
        }
        if !d.classes.is_empty()
            && let Some(b) = self.bare.get_mut(idx)
        {
            *b = false;
        }
        self.add_text(label_bytes)?;
        if let Some(cur) = current {
            let existing_sg = self.nodes.get(idx).and_then(|n| n.subgraph);
            let take = match existing_sg {
                None => true,
                Some(e) => e != cur && self.is_ancestor(e, cur),
            };
            if take && let Some(node) = self.nodes.get_mut(idx) {
                node.subgraph = Some(cur);
            }
        }
        Ok(Some(idx))
    }

    fn commit(&mut self, chain: Chain) -> Result<(), IoError> {
        let mut resolved: Vec<Vec<usize>> = Vec::with_capacity(chain.groups.len());
        for group in &chain.groups {
            let mut ids = Vec::with_capacity(group.len());
            for d in group {
                if let Some(i) = self.touch(d)? {
                    ids.push(i);
                }
            }
            resolved.push(ids);
        }
        for (i, link) in chain.links.iter().enumerate() {
            let (Some(from), Some(to)) = (resolved.get(i), resolved.get(i + 1)) else { continue };
            // Refuse the product before allocating any of it.
            let product = from.len().saturating_mul(to.len());
            if self.items().saturating_add(product) > MAX_DIAGRAM_ITEMS {
                return Err(limit_items());
            }
            let label_len = link.label.as_ref().map_or(0, String::len);
            for &f in from {
                for &t in to {
                    let (Some(fnode), Some(tnode)) = (self.nodes.get(f), self.nodes.get(t)) else { continue };
                    let bytes = fnode.id.len() + tnode.id.len() + label_len;
                    let edge = Edge {
                        from: fnode.id.clone(),
                        to: tnode.id.clone(),
                        from_subgraph: None,
                        to_subgraph: None,
                        label: link.label.clone(),
                        line: link.line,
                        head_start: link.head_start,
                        head_end: link.head_end,
                    };
                    self.add_text(bytes)?;
                    self.edges.push(edge);
                }
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<MermaidGraph, IoError> {
        if !self.stack.is_empty() {
            self.warn("a `subgraph` was never closed with `end`".to_string());
        }

        // Edges to subgraph ids; drop the nodes that only exist because such a link named them.
        let mut sg_by_id: HashMap<&str, usize> = HashMap::new();
        for (i, sg) in self.subgraphs.iter().enumerate() {
            if self.explicit_id.get(i).copied().unwrap_or(false) {
                sg_by_id.entry(sg.id.as_str()).or_insert(i);
            }
        }
        for e in &mut self.edges {
            e.from_subgraph = sg_by_id.get(e.from.as_str()).copied();
            e.to_subgraph = sg_by_id.get(e.to.as_str()).copied();
        }
        let mut keep: Vec<bool> = Vec::with_capacity(self.nodes.len());
        for (i, n) in self.nodes.iter().enumerate() {
            let phantom = sg_by_id.contains_key(n.id.as_str()) && self.bare.get(i).copied().unwrap_or(false);
            keep.push(!phantom);
        }
        drop(sg_by_id);
        let mut nodes: Vec<Node> = Vec::with_capacity(self.nodes.len());
        for (n, k) in std::mem::take(&mut self.nodes).into_iter().zip(keep) {
            if k {
                nodes.push(n);
            }
        }
        let index: HashMap<String, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();

        // Classes from `class` lines.
        for (id, class) in std::mem::take(&mut self.class_assign) {
            if let Some(&i) = index.get(&id)
                && let Some(n) = nodes.get_mut(i)
                && n.classes.len() < MAX_CLASSES_PER_NODE
                && !n.classes.contains(&class)
            {
                n.classes.push(class);
            }
        }
        // Styles: default class, node classes in order, then `style` lines.
        let mut by_node: HashMap<String, Vec<NodeStyle>> = HashMap::new();
        for (id, st) in std::mem::take(&mut self.style_lines) {
            by_node.entry(id).or_default().push(st);
        }
        let default_style = self.class_defs.get("default").cloned();
        for n in &mut nodes {
            let mut style = NodeStyle::default();
            if let Some(d) = &default_style {
                style.merge(d);
            }
            for c in &n.classes {
                if let Some(def) = self.class_defs.get(c) {
                    style.merge(def);
                }
            }
            if let Some(list) = by_node.get(&n.id) {
                for s in list {
                    style.merge(s);
                }
            }
            n.style = if style.is_empty() { None } else { Some(style) };
        }

        // Members.
        for n in &nodes {
            if let Some(sg) = n.subgraph.and_then(|i| self.subgraphs.get_mut(i)) {
                sg.members.push(n.id.clone());
            }
        }

        if nodes.is_empty() && self.subgraphs.is_empty() {
            return Err(IoError::invalid(FormatName::Mermaid, "the flowchart has no nodes"));
        }
        Ok(MermaidGraph {
            direction: self.direction,
            direction_token: self.token,
            nodes,
            edges: self.edges,
            subgraphs: self.subgraphs,
            warnings: self.warnings,
        })
    }
}

/// True when `rest` begins like the continuation of a node statement (`class --> B`), so the first word is a node.
fn starts_like_link(rest: &str) -> bool {
    rest.starts_with(['-', '=', '~', '<', '&', '@', ':', '.']) || rest.starts_with("x-") || rest.starts_with("o-")
}

/// `My Title`, `id`, `id[Title]`, `id["Title"]`, `"Title"`. Returns the id when the text gave one, and the title.
fn parse_subgraph_head(rest: &str) -> Result<(Option<String>, String), IoError> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok((None, String::new()));
    }
    if rest.starts_with('"') {
        return Ok((None, clean_label(rest)));
    }
    let id_end = rest.find(|c: char| !is_id_char(c)).unwrap_or(rest.len());
    let id = rest.get(..id_end).unwrap_or("");
    let after = rest.get(id_end..).unwrap_or("").trim_start();
    if !id.is_empty() {
        if id.chars().count() > MAX_ID_CHARS {
            return Err(limit_id());
        }
        if after.is_empty() {
            return Ok((Some(id.to_string()), id.to_string()));
        }
        if let Some(inner) = after.strip_prefix('[').and_then(|a| a.strip_suffix(']')) {
            let title = clean_label(inner);
            let title = if title.is_empty() { id.to_string() } else { title };
            return Ok((Some(id.to_string()), title));
        }
    }
    Ok((None, clean_label(rest)))
}

/// An id made of the title's letters and digits, for a subgraph whose text gave no id.
fn derive_id(title: &str) -> String {
    let mut id = String::new();
    for c in title.chars().take(MAX_ID_CHARS) {
        if is_id_char(c) {
            id.push(c);
        } else if !id.ends_with('_') {
            id.push('_');
        }
        if id.chars().count() >= 64 {
            break;
        }
    }
    let id = id.trim_matches('_');
    if id.is_empty() { "subgraph".to_string() } else { id.to_string() }
}

// ---------------------------------------------------------------------------------------------------------------
// Style properties

/// Splits at commas that are outside parentheses and quotes.
fn split_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut in_quote = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            '(' if !in_quote => depth += 1,
            ')' if !in_quote => depth = depth.saturating_sub(1),
            ',' if depth == 0 && !in_quote => {
                out.push(s.get(start..i).unwrap_or(""));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s.get(start..).unwrap_or(""));
    out
}

fn parse_color(value: &str) -> Option<Color> {
    let v = value.trim().trim_end_matches("!important").trim();
    Color::parse(v).ok()
}

fn parse_width(value: &str) -> Option<f64> {
    let v = value.trim().trim_end_matches("!important").trim();
    let v = v.strip_suffix("px").unwrap_or(v).trim();
    let w: f64 = v.parse().ok()?;
    (w.is_finite() && (0.0..=MAX_STROKE_WIDTH).contains(&w)).then_some(w)
}

/// `fill:#f9f,stroke:#333,stroke-width:4px,color:#fff`. Unknown keys and bad values are dropped.
fn parse_props(props: &str) -> NodeStyle {
    let mut style = NodeStyle::default();
    for piece in split_commas(props) {
        let Some((key, value)) = piece.split_once(':') else { continue };
        match key.trim().to_ascii_lowercase().as_str() {
            "fill" => style.fill = parse_color(value).or(style.fill),
            "stroke" => style.stroke = parse_color(value).or(style.stroke),
            "color" => style.color = parse_color(value).or(style.color),
            "stroke-width" => style.stroke_width = parse_width(value).or(style.stroke_width),
            _ => {}
        }
    }
    style
}

// ---------------------------------------------------------------------------------------------------------------
// Chains: nodes and links

fn syntax(msg: impl Into<String>) -> PErr {
    PErr::Syntax(msg.into())
}

fn parse_chain(st: &str) -> Result<Chain, PErr> {
    let mut cur = Cur::new(st);
    let mut chain = Chain { groups: Vec::new(), links: Vec::new() };
    let mut count = 0usize;
    loop {
        let group = parse_group(&mut cur, &mut count)?;
        chain.groups.push(group);
        cur.skip_ws();
        if cur.eof() {
            break;
        }
        let link = parse_link(&mut cur)?;
        chain.links.push(link);
    }
    Ok(chain)
}

fn parse_group(cur: &mut Cur<'_>, count: &mut usize) -> Result<Vec<NodeDecl>, PErr> {
    let mut group = Vec::new();
    loop {
        cur.skip_ws();
        let d = parse_node(cur)?;
        *count += 1;
        if *count > MAX_DIAGRAM_ITEMS {
            return Err(PErr::Fatal(limit_items()));
        }
        group.push(d);
        cur.skip_ws();
        if cur.peek() == Some('&') {
            cur.bump();
        } else {
            return Ok(group);
        }
    }
}

fn parse_node(cur: &mut Cur<'_>) -> Result<NodeDecl, PErr> {
    let rest = cur.rest();
    let id_end = rest.find(|c: char| !is_id_char(c)).unwrap_or(rest.len());
    let id = rest.get(..id_end).unwrap_or("");
    if id.is_empty() {
        return Err(syntax(format!("expected a node name at {:?}", shorten(rest, 20))));
    }
    if id.chars().count() > MAX_ID_CHARS {
        return Err(PErr::Fatal(limit_id()));
    }
    cur.advance(id_end);
    let mut d = NodeDecl {
        id: id.to_string(),
        label: None,
        shape: None,
        classes: Vec::new(),
        edge_attrs: false,
        unknown_shape: false,
    };

    if cur.starts_with("@{") {
        cur.advance(1);
        let body = take_braces(cur)?;
        let attrs = parse_node_attrs(body);
        d.edge_attrs = attrs.only_edge_keys;
        d.shape = attrs.shape;
        d.label = attrs.label;
        d.unknown_shape = attrs.unknown_shape;
    } else if let Some((shape, content)) = parse_shape(cur)? {
        d.shape = Some(shape);
        d.label = Some(clean_label(content));
    }
    while cur.starts_with(":::") {
        cur.advance(3);
        let rest = cur.rest();
        let end = rest.find(|c: char| !(is_id_char(c) || c == '-')).unwrap_or(rest.len());
        let name = rest.get(..end).unwrap_or("");
        if name.is_empty() {
            return Err(syntax("expected a class name after `:::`"));
        }
        cur.advance(end);
        if d.classes.len() < MAX_CLASSES_PER_NODE {
            d.classes.push(name.to_string());
        }
    }
    Ok(d)
}

/// With the cursor on `{`: returns the text up to the matching `}` (quotes respected) and moves past it.
fn take_braces<'a>(cur: &mut Cur<'a>) -> Result<&'a str, PErr> {
    let rest = cur.rest();
    let inner = rest.strip_prefix('{').unwrap_or(rest);
    let mut in_quote = false;
    for (i, c) in inner.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            '}' if !in_quote => {
                let body = inner.get(..i).unwrap_or("");
                cur.advance(1 + i + 1);
                return Ok(body);
            }
            _ => {}
        }
    }
    Err(syntax("a `{` was never closed with `}`"))
}

struct NodeAttrs {
    shape: Option<NodeShape>,
    label: Option<String>,
    unknown_shape: bool,
    /// Every key is an edge key (`animate`, `animation`, `curve`): the block belongs to an edge id, not a node.
    only_edge_keys: bool,
}

/// The basic `shape: name, label: "text"` keys of `A@{ ... }`. Other keys are ignored.
fn parse_node_attrs(body: &str) -> NodeAttrs {
    let mut attrs = NodeAttrs { shape: None, label: None, unknown_shape: false, only_edge_keys: true };
    for piece in split_commas(body) {
        let Some((key, value)) = piece.split_once(':') else { continue };
        let key = key.trim().trim_matches('"').to_ascii_lowercase();
        let value = value.trim();
        if !matches!(key.as_str(), "animate" | "animation" | "curve") {
            attrs.only_edge_keys = false;
        }
        match key.as_str() {
            "shape" => {
                attrs.shape = shape_by_name(value.trim_matches('"').trim());
                attrs.unknown_shape = attrs.shape.is_none();
            }
            "label" => attrs.label = Some(clean_label(value)),
            _ => {}
        }
    }
    attrs
}

fn shape_by_name(name: &str) -> Option<NodeShape> {
    Some(match name {
        "rect" | "rectangle" | "proc" | "process" | "div-rect" | "divided-rectangle" => NodeShape::Rectangle,
        "rounded" | "event" => NodeShape::Rounded,
        "stadium" | "terminal" | "pill" => NodeShape::Stadium,
        "fr-rect" | "framed-rectangle" | "subproc" | "subprocess" | "subroutine" => NodeShape::Subroutine,
        "circle" | "circ" => NodeShape::Circle,
        "dbl-circ" | "double-circle" => NodeShape::DoubleCircle,
        "diam" | "diamond" | "decision" | "question" => NodeShape::Diamond,
        "hex" | "hexagon" | "prepare" => NodeShape::Hexagon,
        "cyl" | "cylinder" | "database" | "db" => NodeShape::Cylinder,
        "lean-r" | "lean-right" | "in-out" => NodeShape::Parallelogram,
        "lean-l" | "lean-left" | "out-in" => NodeShape::ParallelogramAlt,
        "trap-b" | "trapezoid-bottom" | "priority" => NodeShape::Trapezoid,
        "trap-t" | "trapezoid-top" | "manual" => NodeShape::TrapezoidAlt,
        "odd" => NodeShape::Asymmetric,
        _ => return None,
    })
}

/// The bracket right after an id. Openers are tried from the longest. One without its closer falls back to a shorter
/// opener (`A[(x) y]` is a rectangle labelled `(x) y`).
fn parse_shape<'a>(cur: &mut Cur<'a>) -> Result<Option<(NodeShape, &'a str)>, PErr> {
    const PLAIN: &[(&str, &str, NodeShape)] = &[
        ("(((", ")))", NodeShape::DoubleCircle),
        ("((", "))", NodeShape::Circle),
        ("([", "])", NodeShape::Stadium),
        ("(", ")", NodeShape::Rounded),
        ("[[", "]]", NodeShape::Subroutine),
        ("[(", ")]", NodeShape::Cylinder),
        ("[", "]", NodeShape::Rectangle),
        ("{{", "}}", NodeShape::Hexagon),
        ("{", "}", NodeShape::Diamond),
        (">", "]", NodeShape::Asymmetric),
    ];
    let rest = cur.rest();
    let first = rest.chars().next();
    if !matches!(first, Some('[' | '(' | '{' | '>')) {
        return Ok(None);
    }
    // The slanted forms are tried just before the plain `[`.
    for (open, close, shape) in PLAIN {
        if *open == "[" {
            for slant in ["[/", "[\\"] {
                if let Some(after) = rest.strip_prefix(slant) {
                    let (par, trap, par_close, trap_close) = if slant == "[/" {
                        (NodeShape::Parallelogram, NodeShape::Trapezoid, "/]", "\\]")
                    } else {
                        (NodeShape::ParallelogramAlt, NodeShape::TrapezoidAlt, "\\]", "/]")
                    };
                    let a = find_closer(after, par_close).map(|i| (i, par, par_close.len()));
                    let b = find_closer(after, trap_close).map(|i| (i, trap, trap_close.len()));
                    let best = match (a, b) {
                        (Some(x), Some(y)) => Some(if x.0 <= y.0 { x } else { y }),
                        (x, y) => x.or(y),
                    };
                    if let Some((i, s, clen)) = best {
                        let content = after.get(..i).unwrap_or("");
                        cur.advance(slant.len() + i + clen);
                        return Ok(Some((s, content)));
                    }
                }
            }
        }
        if let Some(after) = rest.strip_prefix(open)
            && let Some(i) = find_closer(after, close)
        {
            let content = after.get(..i).unwrap_or("");
            cur.advance(open.len() + i + close.len());
            return Ok(Some((*shape, content)));
        }
    }
    // `>` that is not an asymmetric node is left for the link parser; the others are an error.
    if first == Some('>') {
        return Ok(None);
    }
    Err(syntax(format!("a bracket was never closed near {:?}", shorten(rest, 20))))
}

fn is_link_start_char(c: char) -> bool {
    matches!(c, '-' | '=' | '~' | '<' | '.')
}

/// Skips `e1@` in front of a link.
fn skip_edge_id(cur: &mut Cur<'_>) {
    let rest = cur.rest();
    let end = rest.find(|c: char| !is_id_char(c)).unwrap_or(rest.len());
    if end > 0
        && let Some(after) = rest.get(end..)
        && let Some(tail) = after.strip_prefix('@')
        && tail.chars().next().is_some_and(is_link_start_char)
    {
        cur.advance(end + 1);
    }
}

fn head_for(c: char) -> Option<Head> {
    match c {
        '>' => Some(Head::Arrow),
        'x' => Some(Head::Cross),
        'o' => Some(Head::Circle),
        _ => None,
    }
}

fn parse_link(cur: &mut Cur<'_>) -> Result<LinkSpec, PErr> {
    cur.skip_ws();
    skip_edge_id(cur);
    let fail = |cur: &Cur<'_>| syntax(format!("expected a link such as `-->` at {:?}", shorten(cur.rest(), 20)));

    let mut head_start = Head::None;
    match (cur.peek(), cur.peek2()) {
        (Some('<'), Some('-' | '=')) => {
            head_start = Head::Arrow;
            cur.bump();
        }
        (Some(c @ ('x' | 'o')), Some('-' | '=')) => {
            head_start = if c == 'x' { Head::Cross } else { Head::Circle };
            cur.bump();
        }
        _ => {}
    }

    let mut spec = match cur.peek() {
        Some('~') => {
            let n = cur.eat_run('~');
            if n < 3 || head_start != Head::None {
                return Err(fail(cur));
            }
            LinkSpec { line: Line::Invisible, head_start, head_end: Head::None, label: None }
        }
        Some('=') => parse_stroke(cur, '=', Line::Thick, head_start).ok_or_else(|| fail(cur))?,
        Some('-') if cur.peek2() == Some('.') => parse_dotted(cur, head_start).ok_or_else(|| fail(cur))?,
        Some('-') => parse_stroke(cur, '-', Line::Solid, head_start).ok_or_else(|| fail(cur))?,
        _ => return Err(fail(cur)),
    };

    // A pipe label after the link: `-->|text|`.
    cur.skip_ws();
    if cur.peek() == Some('|') {
        cur.bump();
        let rest = cur.rest();
        let Some(i) = find_closer(rest, "|") else {
            return Err(syntax("a `|` label is never closed"));
        };
        let text = clean_label(rest.get(..i).unwrap_or(""));
        cur.advance(i + 1);
        spec.label = Some(text);
    }
    Ok(spec)
}

/// `--`, `-->`, `---`, `--x`, `-- text -->` (and the same with `=`). The cursor is on the first dash.
fn parse_stroke(cur: &mut Cur<'_>, c: char, line: Line, head_start: Head) -> Option<LinkSpec> {
    let n = cur.eat_run(c);
    if n < 2 {
        // `->` is no Mermaid link but the old importer drew it as an arrow.
        if n == 1 && c == '-' && cur.peek() == Some('>') {
            cur.bump();
            return Some(LinkSpec { line, head_start, head_end: Head::Arrow, label: None });
        }
        return None;
    }
    if let Some(h) = cur.peek().and_then(head_for) {
        cur.bump();
        return Some(LinkSpec { line, head_start, head_end: h, label: None });
    }
    if n >= 3 {
        return Some(LinkSpec { line, head_start, head_end: Head::None, label: None });
    }
    // Exactly two: either text follows (`-- text -->`) or, with a start head, this is `<--`.
    let rest = cur.rest();
    if let Some((label_end, close_end, head_end)) = find_embedded_close(rest, c) {
        let label = clean_label(rest.get(..label_end).unwrap_or(""));
        cur.advance(close_end);
        return Some(LinkSpec { line, head_start, head_end, label: Some(label) });
    }
    if head_start != Head::None {
        return Some(LinkSpec { line, head_start, head_end: Head::None, label: None });
    }
    None
}

/// In text after `-- ` finds the closing run of `c` (`-->`, `---`, `--x`). Returns the end of the label, the end of
/// the closing link and its end head. Quoted text is skipped.
fn find_embedded_close(rest: &str, c: char) -> Option<(usize, usize, Head)> {
    let mut chars = rest.char_indices().peekable();
    let mut in_quote = false;
    while let Some((i, ch)) = chars.next() {
        if ch == '"' {
            in_quote = !in_quote;
            continue;
        }
        if in_quote || ch != c {
            continue;
        }
        let mut run = 1;
        let mut after = i + ch.len_utf8();
        while let Some(&(j, nc)) = chars.peek() {
            if nc != c {
                break;
            }
            run += 1;
            after = j + nc.len_utf8();
            chars.next();
        }
        if run < 2 {
            continue;
        }
        let tail = rest.get(after..).unwrap_or("");
        if let Some(h) = tail.chars().next().and_then(head_for) {
            let len = tail.chars().next().map_or(0, char::len_utf8);
            return Some((i, after + len, h));
        }
        if run >= 3 {
            return Some((i, after, Head::None));
        }
    }
    None
}

/// `-.->`, `-.-`, `-..->`, `-.-x`, `-. text .->`. The cursor is on the first dash.
fn parse_dotted(cur: &mut Cur<'_>, head_start: Head) -> Option<LinkSpec> {
    cur.bump(); // '-'
    let dots = cur.eat_run('.');
    if dots == 0 {
        return None;
    }
    if cur.peek() == Some('-') {
        cur.bump();
        let head_end = match cur.peek().and_then(head_for) {
            Some(h) => {
                cur.bump();
                h
            }
            None => Head::None,
        };
        return Some(LinkSpec { line: Line::Dotted, head_start, head_end, label: None });
    }
    // Text, then `.->` or `.-`.
    let rest = cur.rest();
    let mut in_quote = false;
    let mut iter = rest.char_indices().peekable();
    while let Some((i, ch)) = iter.next() {
        if ch == '"' {
            in_quote = !in_quote;
            continue;
        }
        if in_quote || ch != '.' {
            continue;
        }
        let mut after = i + 1;
        while let Some(&(j, nc)) = iter.peek() {
            if nc != '.' {
                break;
            }
            after = j + 1;
            iter.next();
        }
        let tail = rest.get(after..).unwrap_or("");
        if let Some(t) = tail.strip_prefix('-') {
            let (head_end, len) = match t.chars().next().and_then(head_for) {
                Some(h) => (h, 1 + t.chars().next().map_or(0, char::len_utf8)),
                None => (Head::None, 1),
            };
            let label = clean_label(rest.get(..i).unwrap_or(""));
            cur.advance(after + len);
            return Some(LinkSpec { line: Line::Dotted, head_start, head_end, label: Some(label) });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(s: &str) -> Option<(LinkSpec, String)> {
        let mut cur = Cur::new(s);
        match parse_link(&mut cur) {
            Ok(l) => Some((l, cur.rest().to_string())),
            Err(_) => None,
        }
    }

    fn check(s: &str, line: Line, hs: Head, he: Head, label: Option<&str>, rest: &str) {
        let (l, r) = link(s).unwrap_or_else(|| panic!("no link in {s:?}"));
        assert_eq!((l.line, l.head_start, l.head_end), (line, hs, he), "{s}");
        assert_eq!(l.label.as_deref(), label, "{s}");
        assert_eq!(r.trim(), rest, "{s}");
    }

    #[test]
    fn link_forms() {
        use Head::{Arrow, Circle, Cross, None as N};
        use Line::{Dotted, Invisible, Solid, Thick};
        check("-->B", Solid, N, Arrow, None, "B");
        check("--- B", Solid, N, N, None, "B");
        check("---->B", Solid, N, Arrow, None, "B");
        check("-.->B", Dotted, N, Arrow, None, "B");
        check("-.-B", Dotted, N, N, None, "B");
        check("-..->B", Dotted, N, Arrow, None, "B");
        check("==>B", Thick, N, Arrow, None, "B");
        check("===B", Thick, N, N, None, "B");
        check("====>B", Thick, N, Arrow, None, "B");
        check("~~~B", Invisible, N, N, None, "B");
        check("--xB", Solid, N, Cross, None, "B");
        check("--oB", Solid, N, Circle, None, "B");
        check("<-->B", Solid, Arrow, Arrow, None, "B");
        check("x--xB", Solid, Cross, Cross, None, "B");
        check("o--oB", Solid, Circle, Circle, None, "B");
        check("<-.->B", Dotted, Arrow, Arrow, None, "B");
        check("<==>B", Thick, Arrow, Arrow, None, "B");
        check("<-- B", Solid, Arrow, N, None, "B");
        check("-->|text| B", Solid, N, Arrow, Some("text"), "B");
        check("-- text --> B", Solid, N, Arrow, Some("text"), "B");
        check("-- \"a -- b\" --> B", Solid, N, Arrow, Some("a -- b"), "B");
        check("-- text --- B", Solid, N, N, Some("text"), "B");
        check("-. text .-> B", Dotted, N, Arrow, Some("text"), "B");
        check("== text ==> B", Thick, N, Arrow, Some("text"), "B");
        check("e1@--> B", Solid, N, Arrow, None, "B");
        check("-> B", Solid, N, Arrow, None, "B");
    }

    #[test]
    fn bad_links() {
        assert!(link("- B").is_none());
        assert!(link("~~ B").is_none());
        assert!(link("-- B").is_none());
        assert!(link("-->|oops B").is_none());
        assert!(link("B").is_none());
    }

    #[test]
    fn shapes() {
        let cases: &[(&str, NodeShape, &str)] = &[
            ("A[x]", NodeShape::Rectangle, "x"),
            ("A(x)", NodeShape::Rounded, "x"),
            ("A([x])", NodeShape::Stadium, "x"),
            ("A[[x]]", NodeShape::Subroutine, "x"),
            ("A((x))", NodeShape::Circle, "x"),
            ("A(((x)))", NodeShape::DoubleCircle, "x"),
            ("A{x}", NodeShape::Diamond, "x"),
            ("A{{x}}", NodeShape::Hexagon, "x"),
            ("A[(x)]", NodeShape::Cylinder, "x"),
            ("A[/x/]", NodeShape::Parallelogram, "x"),
            ("A[\\x\\]", NodeShape::ParallelogramAlt, "x"),
            ("A[/x\\]", NodeShape::Trapezoid, "x"),
            ("A[\\x/]", NodeShape::TrapezoidAlt, "x"),
            ("A>x]", NodeShape::Asymmetric, "x"),
            ("A[(x) y]", NodeShape::Rectangle, "(x) y"),
            ("A[/path/to]", NodeShape::Rectangle, "/path/to"),
            ("A[\"a]b\"]", NodeShape::Rectangle, "a]b"),
        ];
        for (src, shape, label) in cases {
            let mut cur = Cur::new(src);
            let d = parse_node(&mut cur).unwrap_or_else(|_| panic!("{src}"));
            assert_eq!((d.shape, d.label.as_deref()), (Some(*shape), Some(*label)), "{src}");
            assert!(cur.eof(), "{src}");
        }
    }

    #[test]
    fn props() {
        let s = parse_props("fill:#f9f,stroke:#333,stroke-width:4px,color:red,unknown:1");
        assert_eq!(s.fill, Some(Color::rgb(0xff, 0x99, 0xff)));
        assert_eq!(s.stroke, Some(Color::rgb(0x33, 0x33, 0x33)));
        assert_eq!(s.stroke_width, Some(4.0));
        assert_eq!(s.color, None);
        assert!(parse_props("fill:nope,stroke-width:-3,stroke:rgb(1,2,3)").is_empty());
    }

    #[test]
    fn subgraph_heads() {
        let h = |s: &str| parse_subgraph_head(s).unwrap_or((None, "ERR".into()));
        assert_eq!(h("My Title"), (None, "My Title".to_string()));
        assert_eq!(h("id [Title here]"), (Some("id".into()), "Title here".to_string()));
        assert_eq!(h("id[\"Q\"]"), (Some("id".into()), "Q".to_string()));
        assert_eq!(h("id"), (Some("id".into()), "id".to_string()));
        assert_eq!(h("\"Quoted title\""), (None, "Quoted title".to_string()));
        assert_eq!(derive_id("My Title!"), "My_Title");
        assert_eq!(derive_id("!!"), "subgraph");
    }
}
