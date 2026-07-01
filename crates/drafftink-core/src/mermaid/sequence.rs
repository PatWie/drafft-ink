//! Sequence-diagram parser and shape builder.
//!
//! Translates Mermaid `sequenceDiagram` source into native shapes: a labeled
//! box per participant, a dashed vertical lifeline beneath each box, an arrow
//! per message (solid or dashed depending on the Mermaid arrow token), and a
//! filled note box per `Note` directive. Messages and notes are stacked
//! top-to-bottom in source order.

use super::{
    BOX_FILL, BOX_PADDING_X, BOX_PADDING_Y, LABEL_FONT_SIZE, MIN_BOX_WIDTH, NOTE_FILL, base_style,
    centered_text, line_shape, measure_text, normalize_label, rect_shape,
};
use crate::shapes::{Arrow, PathStyle, Shape, StrokeStyle};
use kurbo::Point;

// -----------------------------------------------------------------------------
// Layout constants (see `mermaid::mod` for the shared ones).
// -----------------------------------------------------------------------------

/// Height (pixels) of a participant header box.
const PARTICIPANT_BOX_HEIGHT: f64 = 44.0;

/// Horizontal gap (pixels) between adjacent participant columns.
const COLUMN_GAP: f64 = 64.0;

/// Vertical distance (pixels) advanced for each ordinary message.
const MESSAGE_GAP_Y: f64 = 56.0;

/// Vertical gap (pixels) between the participant boxes and the first event.
const TOP_MARGIN: f64 = 56.0;

/// Vertical gap (pixels) below the last event before the lifelines end.
const BOTTOM_MARGIN: f64 = 40.0;

/// Distance (pixels) a message label floats above its arrow.
const MESSAGE_LABEL_OFFSET: f64 = 10.0;

/// Minimum height (pixels) of a note box.
const NOTE_MIN_HEIGHT: f64 = 40.0;

/// Vertical gap (pixels) added after a note box.
const NOTE_GAP_Y: f64 = 24.0;

/// Width (pixels) of the horizontal arm of a self-message loop.
const SELF_LOOP_WIDTH: f64 = 72.0;

/// Height (pixels) of the vertical arm of a self-message loop.
const SELF_LOOP_HEIGHT: f64 = 40.0;

/// Corner radius (pixels) for participant and note boxes.
const BOX_CORNER_RADIUS: f64 = 4.0;

/// Mermaid message arrow tokens, longest first so prefixes never shadow the
/// longer form (e.g. `-->>` must be tried before `->>`).
const ARROW_TOKENS: &[&str] = &[
    "<<-->>", "<<->>", "-->>", "--x", "--)", "-->", "->>", "-x", "-)", "->",
];

// -----------------------------------------------------------------------------
// Parsed model.
// -----------------------------------------------------------------------------

/// A participant column, identified by its Mermaid alias with a display label.
struct Participant {
    id: String,
    label: String,
}

/// One message between two participant columns (indices into the participant
/// list). `dashed` reflects a Mermaid `--` arrow (a reply/return message).
struct Message {
    from: usize,
    to: usize,
    label: String,
    dashed: bool,
}

/// Where a note is anchored relative to its participant span.
enum NotePlacement {
    Over,
    LeftOf,
    RightOf,
}

/// A note spanning one or more participant columns (inclusive index range).
struct Note {
    start: usize,
    end: usize,
    placement: NotePlacement,
    label: String,
}

/// A time-ordered diagram event.
enum Event {
    Message(Message),
    Note(Note),
}

/// The fully parsed sequence diagram.
struct SequenceDiagram {
    participants: Vec<Participant>,
    events: Vec<Event>,
}

// -----------------------------------------------------------------------------
// Public entry.
// -----------------------------------------------------------------------------

/// Parse `body` and build the shapes for a sequence diagram. Returns an empty
/// vector if there is nothing drawable.
pub(super) fn build(body: &str) -> Vec<Shape> {
    let diagram = parse(body);
    if diagram.participants.is_empty() {
        return Vec::new();
    }
    layout(&diagram)
}

// -----------------------------------------------------------------------------
// Parsing.
// -----------------------------------------------------------------------------

/// Parse the diagram body into participants and time-ordered events. Unknown or
/// unsupported lines are skipped so arbitrary input never aborts the import.
fn parse(body: &str) -> SequenceDiagram {
    let mut participants: Vec<Participant> = Vec::new();
    let mut events: Vec<Event> = Vec::new();

    for raw in body.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() || line == "sequenceDiagram" {
            continue;
        }

        if let Some(rest) = keyword(line, "participant").or_else(|| keyword(line, "actor")) {
            parse_participant(rest, &mut participants);
        } else if let Some(rest) = keyword(line, "Note").or_else(|| keyword(line, "note")) {
            if let Some(note) = parse_note(rest, &mut participants) {
                events.push(Event::Note(note));
            }
        } else if let Some(message) = parse_message(line, &mut participants) {
            events.push(Event::Message(message));
        }
        // Any other construct (loop/alt/opt/activate/...) is intentionally
        // skipped: it carries no standalone geometry in this importer.
    }

    SequenceDiagram {
        participants,
        events,
    }
}

/// Strip a trailing Mermaid `%% ...` comment from a line.
fn strip_inline_comment(line: &str) -> &str {
    match line.find("%%") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// If `line` begins with `word` followed by whitespace, return the remainder.
fn keyword<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(word)?;
    if rest.is_empty() {
        return Some(rest);
    }
    // Require a word boundary so `Notebook` is not mistaken for `Note`.
    if rest.starts_with(char::is_whitespace) {
        Some(rest.trim_start())
    } else {
        None
    }
}

/// Parse a `participant X as Label` / `participant X` declaration.
fn parse_participant(rest: &str, participants: &mut Vec<Participant>) {
    let (id, label) = match rest.split_once(" as ") {
        Some((id, label)) => (id.trim(), label.trim()),
        None => (rest.trim(), rest.trim()),
    };
    if id.is_empty() {
        return;
    }
    intern_participant(participants, id, Some(label));
}

/// Find or create a participant by id, optionally setting an explicit label.
/// Returns the participant's column index.
fn intern_participant(
    participants: &mut Vec<Participant>,
    id: &str,
    explicit_label: Option<&str>,
) -> usize {
    if let Some(idx) = participants.iter().position(|p| p.id == id) {
        if let Some(label) = explicit_label {
            participants[idx].label = normalize_label(label);
        }
        return idx;
    }
    let label = normalize_label(explicit_label.unwrap_or(id));
    participants.push(Participant {
        id: id.to_string(),
        label,
    });
    participants.len() - 1
}

/// Parse a `Note over X[,Y]: text` / `Note left of X: text` /
/// `Note right of X: text` directive.
fn parse_note(rest: &str, participants: &mut Vec<Participant>) -> Option<Note> {
    let (anchor, label) = rest.split_once(':').unwrap_or((rest, ""));
    let label = normalize_label(label);

    let (placement, targets) = if let Some(t) = keyword(anchor, "over") {
        (NotePlacement::Over, t)
    } else if let Some(t) = anchor.strip_prefix("left of") {
        (NotePlacement::LeftOf, t.trim())
    } else if let Some(t) = anchor.strip_prefix("right of") {
        (NotePlacement::RightOf, t.trim())
    } else {
        return None;
    };

    let mut indices: Vec<usize> = targets
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| intern_participant(participants, id, None))
        .collect();
    if indices.is_empty() {
        return None;
    }
    indices.sort_unstable();
    Some(Note {
        start: *indices.first().unwrap(),
        end: *indices.last().unwrap(),
        placement,
        label,
    })
}

/// Parse a message line of the form `A <arrow> B : label`.
fn parse_message(line: &str, participants: &mut Vec<Participant>) -> Option<Message> {
    let (endpoints, label) = line.split_once(':').unwrap_or((line, ""));
    let (start, token_len, dashed) = find_arrow(endpoints)?;

    let from_id = endpoints[..start].trim();
    let to_id = endpoints[start + token_len..].trim();
    if from_id.is_empty() || to_id.is_empty() {
        return None;
    }

    let from = intern_participant(participants, from_id, None);
    let to = intern_participant(participants, to_id, None);
    Some(Message {
        from,
        to,
        label: normalize_label(label),
        dashed,
    })
}

/// Locate the leftmost message-arrow token in `s`. Returns its byte offset, its
/// length, and whether it is a dashed (return) arrow.
fn find_arrow(s: &str) -> Option<(usize, usize, bool)> {
    let mut best: Option<(usize, usize, bool)> = None;
    for token in ARROW_TOKENS {
        if let Some(idx) = s.find(token) {
            let candidate = (idx, token.len(), token.contains("--"));
            best = Some(match best {
                // Prefer the leftmost token; on a tie prefer the longer one so
                // `-->` wins over the `->` embedded within it.
                Some(current) if current.0 < idx => current,
                Some(current) if current.0 == idx && current.1 >= token.len() => current,
                _ => candidate,
            });
        }
    }
    best
}

// -----------------------------------------------------------------------------
// Layout / shape building.
// -----------------------------------------------------------------------------

/// Turn a parsed diagram into positioned shapes, ordered back-to-front:
/// lifelines, then events, then participant boxes on top.
fn layout(diagram: &SequenceDiagram) -> Vec<Shape> {
    let count = diagram.participants.len();

    // Box width per participant, and a uniform column pitch that fits the widest.
    let box_widths: Vec<f64> = diagram
        .participants
        .iter()
        .map(|p| participant_box_width(&p.label))
        .collect();
    let max_box_width = box_widths.iter().copied().fold(MIN_BOX_WIDTH, f64::max);
    let column_pitch = max_box_width + COLUMN_GAP;
    let center_x: Vec<f64> = (0..count)
        .map(|i| i as f64 * column_pitch + column_pitch / 2.0)
        .collect();

    // Assign a vertical position to every event in source order.
    let mut shapes: Vec<Shape> = Vec::new();
    let mut y = PARTICIPANT_BOX_HEIGHT + TOP_MARGIN;
    let mut event_shapes: Vec<Shape> = Vec::new();
    for event in &diagram.events {
        match event {
            Event::Message(message) => {
                y = build_message(message, &center_x, y, &mut event_shapes);
            }
            Event::Note(note) => {
                y = build_note(
                    note,
                    &center_x,
                    &box_widths,
                    max_box_width,
                    y,
                    &mut event_shapes,
                );
            }
        }
    }
    let lifeline_bottom = y + BOTTOM_MARGIN;

    // 1) Lifelines (drawn first, behind everything else).
    for &cx in &center_x {
        let start = Point::new(cx, PARTICIPANT_BOX_HEIGHT);
        let end = Point::new(cx, lifeline_bottom);
        shapes.push(Shape::Line(line_shape(start, end, StrokeStyle::Dashed)));
    }

    // 2) Messages and notes.
    shapes.append(&mut event_shapes);

    // 3) Participant boxes and their labels (on top of the lifelines).
    for (i, participant) in diagram.participants.iter().enumerate() {
        let box_width = box_widths[i];
        let left = center_x[i] - box_width / 2.0;
        shapes.push(Shape::Rectangle(rect_shape(
            left,
            0.0,
            box_width,
            PARTICIPANT_BOX_HEIGHT,
            BOX_CORNER_RADIUS,
            Some(BOX_FILL),
        )));
        let center = Point::new(center_x[i], PARTICIPANT_BOX_HEIGHT / 2.0);
        shapes.push(Shape::Text(centered_text(center, &participant.label)));
    }

    shapes
}

/// Width of a participant box sized to its label.
fn participant_box_width(label: &str) -> f64 {
    let (w, _) = measure_text(label, LABEL_FONT_SIZE);
    (w + 2.0 * BOX_PADDING_X).max(MIN_BOX_WIDTH)
}

/// Emit the shapes for one message and return the next free `y`.
fn build_message(message: &Message, center_x: &[f64], y: f64, out: &mut Vec<Shape>) -> f64 {
    if message.from == message.to {
        return build_self_message(message, center_x, y, out);
    }

    let x_from = center_x[message.from];
    let x_to = center_x[message.to];
    out.push(Shape::Arrow(arrow_shape(
        vec![Point::new(x_from, y), Point::new(x_to, y)],
        message.dashed,
    )));

    if !message.label.is_empty() {
        let (_, label_h) = measure_text(&message.label, LABEL_FONT_SIZE);
        let center = Point::new(
            (x_from + x_to) / 2.0,
            y - MESSAGE_LABEL_OFFSET - label_h / 2.0,
        );
        out.push(Shape::Text(centered_text(center, &message.label)));
    }
    y + MESSAGE_GAP_Y
}

/// Emit the shapes for a self-message (source == target) as a small loop and
/// return the next free `y`.
fn build_self_message(message: &Message, center_x: &[f64], y: f64, out: &mut Vec<Shape>) -> f64 {
    let cx = center_x[message.from];
    let loop_points = vec![
        Point::new(cx, y),
        Point::new(cx + SELF_LOOP_WIDTH, y),
        Point::new(cx + SELF_LOOP_WIDTH, y + SELF_LOOP_HEIGHT),
        Point::new(cx, y + SELF_LOOP_HEIGHT),
    ];
    out.push(Shape::Arrow(arrow_shape(loop_points, message.dashed)));

    if !message.label.is_empty() {
        let (label_w, _) = measure_text(&message.label, LABEL_FONT_SIZE);
        let center = Point::new(
            cx + SELF_LOOP_WIDTH + label_w / 2.0 + BOX_PADDING_X,
            y + SELF_LOOP_HEIGHT / 2.0,
        );
        out.push(Shape::Text(centered_text(center, &message.label)));
    }
    y + SELF_LOOP_HEIGHT + MESSAGE_GAP_Y
}

/// Emit the shapes for a note and return the next free `y`.
fn build_note(
    note: &Note,
    center_x: &[f64],
    box_widths: &[f64],
    max_box_width: f64,
    y: f64,
    out: &mut Vec<Shape>,
) -> f64 {
    let (label_w, label_h) = measure_text(&note.label, LABEL_FONT_SIZE);
    let note_h = (label_h + 2.0 * BOX_PADDING_Y).max(NOTE_MIN_HEIGHT);

    let span_left = center_x[note.start] - box_widths[note.start] / 2.0;
    let span_right = center_x[note.end] + box_widths[note.end] / 2.0;
    let span_width = (span_right - span_left).max(max_box_width);
    let note_w = (label_w + 2.0 * BOX_PADDING_X).max(span_width);

    // Horizontal center depends on the placement relative to the anchor span.
    let anchor_center = (center_x[note.start] + center_x[note.end]) / 2.0;
    let center_col_x = match note.placement {
        NotePlacement::Over => anchor_center,
        NotePlacement::LeftOf => center_x[note.start] - max_box_width / 2.0 - note_w / 2.0,
        NotePlacement::RightOf => center_x[note.end] + max_box_width / 2.0 + note_w / 2.0,
    };

    let top = y - note_h / 2.0;
    out.push(Shape::Rectangle(rect_shape(
        center_col_x - note_w / 2.0,
        top,
        note_w,
        note_h,
        BOX_CORNER_RADIUS,
        Some(NOTE_FILL),
    )));
    out.push(Shape::Text(centered_text(
        Point::new(center_col_x, y),
        &note.label,
    )));

    y + note_h / 2.0 + NOTE_GAP_Y + MESSAGE_GAP_Y / 2.0
}

/// Build a message arrow through the given points with the given dashed state.
fn arrow_shape(points: Vec<Point>, dashed: bool) -> Arrow {
    let mut arrow = Arrow::from_points(points, PathStyle::Direct);
    arrow.stroke_style = if dashed {
        StrokeStyle::Dashed
    } else {
        StrokeStyle::Solid
    };
    arrow.style = base_style(None);
    arrow
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A multi-participant example using a neutral pizza-shop order flow.
    const EXAMPLE: &str = "sequenceDiagram\n\
        participant C as Customer\n\
        participant CA as Cashier\n\
        participant KI as Kitchen\n\
        participant OV as Oven\n\
        participant CO as Courier\n\
        C->>CA: place an order\n\
        CA-->>C: order confirmed (paid)\n\
        Note over C: waiting\n\
        CA->>KI: send the ticket to the kitchen\n\
        CA->>OV: reserve the oven\n\
        KI-->>OV: dough is ready\n\
        OV->>CO: hand off the boxed pizza\n\
        KI-->>C: order ready (baked)\n\
        Note over C: enjoy the next slice\n";

    #[test]
    fn parses_all_participants_in_order() {
        let diagram = parse(EXAMPLE);
        let ids: Vec<&str> = diagram.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["C", "CA", "KI", "OV", "CO"]);
        assert_eq!(diagram.participants[0].label, "Customer");
        assert_eq!(diagram.participants[1].label, "Cashier");
    }

    #[test]
    fn parses_messages_and_notes() {
        let diagram = parse(EXAMPLE);
        let messages = diagram
            .events
            .iter()
            .filter(|e| matches!(e, Event::Message(_)))
            .count();
        let notes = diagram
            .events
            .iter()
            .filter(|e| matches!(e, Event::Note(_)))
            .count();
        assert_eq!(messages, 7);
        assert_eq!(notes, 2);
    }

    #[test]
    fn detects_dashed_reply_arrows() {
        let diagram = parse(EXAMPLE);
        // "GP-->>C" is a dashed reply; "C->>GP" is solid.
        let first_two: Vec<bool> = diagram
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Message(m) => Some(m.dashed),
                _ => None,
            })
            .take(2)
            .collect();
        assert_eq!(first_two, [false, true]);
    }

    #[test]
    fn find_arrow_prefers_longer_dashed_token() {
        let (_, len, dashed) = find_arrow("A-->>B").expect("arrow present");
        assert_eq!(len, "-->>".len());
        assert!(dashed);
        let (_, len, dashed) = find_arrow("A->>B").expect("arrow present");
        assert_eq!(len, "->>".len());
        assert!(!dashed);
    }

    #[test]
    fn build_produces_boxes_lifelines_and_arrows() {
        let shapes = build(EXAMPLE);
        let rects = shapes
            .iter()
            .filter(|s| matches!(s, Shape::Rectangle(_)))
            .count();
        let lines = shapes
            .iter()
            .filter(|s| matches!(s, Shape::Line(_)))
            .count();
        let arrows = shapes
            .iter()
            .filter(|s| matches!(s, Shape::Arrow(_)))
            .count();
        // 5 participant boxes + 2 note boxes.
        assert_eq!(rects, 7);
        // One lifeline per participant.
        assert_eq!(lines, 5);
        // One arrow per message.
        assert_eq!(arrows, 7);
    }

    #[test]
    fn self_message_is_supported() {
        let src = "sequenceDiagram\nA->>A: retry\n";
        let shapes = build(src);
        assert_eq!(
            shapes
                .iter()
                .filter(|s| matches!(s, Shape::Arrow(_)))
                .count(),
            1
        );
    }
}
