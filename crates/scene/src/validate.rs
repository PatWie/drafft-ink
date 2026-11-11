//! Invariants as functions with typed errors.
//!
//! [`validate_element`] checks one element on its own: ranges, finiteness, sizes, ids. [`validate_document`] and
//! [`validate_scene`] check the relations between elements: bound text containers, binding targets, frames, group
//! stacks, z-order keys, image blobs.
//!
//! Every function accepts hostile input and returns errors; none panics. A dangling reference to an id that does
//! not exist is an error. A reference to a deleted (tombstoned) element is only a warning, because concurrent edits
//! legitimately produce it and the owner of the reference resolves it as "not attached".

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use drafftink_geom::aabb;

use crate::document::Document;
use crate::element::{Element, ElementKind, Kind, LinearData, Transform};
use crate::id::{ElementId, FileId, GroupId};
use crate::limits::*;
use crate::store::SceneRead;
use crate::zindex::{MAX_INDEX_LEN, ZIndex};

/// A rule an element breaks on its own.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ElementError {
    /// A number is NaN or infinite.
    #[error("`{field}` must be a finite number")]
    NotFinite {
        /// Field name.
        field: &'static str,
    },
    /// A number is outside its allowed range.
    #[error("`{field}` is {value}, and must be between {min} and {max}")]
    OutOfRange {
        /// Field name.
        field: &'static str,
        /// Offending value.
        value: f64,
        /// Smallest allowed.
        min: f64,
        /// Largest allowed.
        max: f64,
    },
    /// A list or text is longer than the limit.
    #[error("`{field}` has {len} entries, and the limit is {max}")]
    TooLong {
        /// Field name.
        field: &'static str,
        /// Actual length.
        len: usize,
        /// Limit.
        max: usize,
    },
    /// A line, arrow or stroke has too few points.
    #[error("{} needs at least {min} points, and has {have}", crate::words::Article::new(.kind))]
    TooFewPoints {
        /// Kind name.
        kind: &'static str,
        /// Smallest allowed.
        min: usize,
        /// Actual count.
        have: usize,
    },
    /// The element box does not match the bounding box of its points. Call `Element::refit_points`.
    #[error("the box {w} x {h} does not match the points, which span {pw} x {ph} from the origin; call `refit_points`")]
    BoxMismatch {
        /// Stored width.
        w: f64,
        /// Stored height.
        h: f64,
        /// Width of the points.
        pw: f64,
        /// Height of the points.
        ph: f64,
    },
    /// The points do not start at the local origin.
    #[error("the points must start at the local origin (smallest x and y are 0), but start at ({x}, {y})")]
    PointsNotAtOrigin {
        /// Smallest x.
        x: f64,
        /// Smallest y.
        y: f64,
    },
    /// Pressures are neither empty nor one per point.
    #[error("freehand `pressures` has {pressures} entries for {points} points; use none or one per point")]
    PressureCount {
        /// Number of pressures.
        pressures: usize,
        /// Number of points.
        points: usize,
    },
    /// A binding or label refers to the element itself.
    #[error("`{field}` refers to the element itself")]
    SelfReference {
        /// Field name.
        field: &'static str,
    },
    /// An element is in a group more than once.
    #[error("group {0} appears more than once in `group_ids`")]
    DuplicateGroup(GroupId),
    /// Group stack too deep.
    #[error("`group_ids` nests {len} groups, and the limit is {max}")]
    GroupDepth {
        /// Depth.
        len: usize,
        /// Limit.
        max: usize,
    },
    /// Fixed elbow segments are listed out of order or out of range.
    #[error("fixed segment index {index} is invalid: indices must increase and lie within 1..{max}")]
    FixedSegment {
        /// Offending index.
        index: u32,
        /// One past the largest allowed.
        max: usize,
    },
    /// An option that only makes sense on another kind or path type.
    #[error("{0}")]
    NotApplicable(&'static str),
    /// Text runs are unsorted, overlapping, empty or past the end of the text.
    #[error(
        "text run {start}..{end} is invalid: runs must be sorted, not overlap, be non-empty and end within the {len} characters"
    )]
    TextRun {
        /// Run start.
        start: u32,
        /// Run end.
        end: u32,
        /// Characters in the text.
        len: usize,
    },
    /// A name or label has a forbidden shape.
    #[error("`{field}` must be non-empty, free of control characters, and at most {max} long")]
    BadText {
        /// Field name.
        field: &'static str,
        /// Limit.
        max: usize,
    },
    /// The mime type is not an image type.
    #[error("`mime` must look like image/png, and is {0:?}")]
    BadMime(String),
    /// The z-order key is too long to keep. Rebalance the keys.
    #[error("the z-order key is {len} characters long, and the limit is {MAX_INDEX_LEN}; rebalance the keys")]
    IndexTooLong {
        /// Key length.
        len: usize,
    },
}

fn finite(field: &'static str, v: f64) -> Result<(), ElementError> {
    if v.is_finite() { Ok(()) } else { Err(ElementError::NotFinite { field }) }
}

fn ranged(field: &'static str, v: f64, min: f64, max: f64) -> Result<(), ElementError> {
    finite(field, v)?;
    if v < min || v > max { Err(ElementError::OutOfRange { field, value: v, min, max }) } else { Ok(()) }
}

/// Is `name` a valid element or document name? It has 1 to [`MAX_NAME_CHARS`] characters and no control
/// characters. Stores and importers use this one rule.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= MAX_NAME_CHARS && !name.chars().any(char::is_control)
}

/// Is `mime` an image media type like `image/png`? At most 64 bytes of letters, digits and `/ + . -`, starting
/// with `image/` and naming a subtype. Stores and importers use this one rule.
pub fn is_image_media_type(mime: &str) -> bool {
    mime.len() <= 64
        && mime.len() > "image/".len()
        && mime.starts_with("image/")
        && mime.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'+' | b'.' | b'-'))
}

fn text_ok(field: &'static str, s: &str) -> Result<(), ElementError> {
    if is_valid_name(s) { Ok(()) } else { Err(ElementError::BadText { field, max: MAX_NAME_CHARS }) }
}

/// Checks the box and angle of a transform. A move or a turn changes nothing else about an element, so this is
/// all such a change has to check.
pub fn validate_transform(t: &Transform) -> Result<(), ElementError> {
    let mut errors = Vec::new();
    transform_errors(t, &mut errors);
    errors.into_iter().next().map_or(Ok(()), Err)
}

fn transform_errors(t: &Transform, out: &mut Vec<ElementError>) {
    let checks = [
        ranged("transform.x", t.x, -MAX_COORD, MAX_COORD),
        ranged("transform.y", t.y, -MAX_COORD, MAX_COORD),
        ranged("transform.w", t.w, 0.0, MAX_COORD),
        ranged("transform.h", t.h, 0.0, MAX_COORD),
        finite("transform.angle", t.angle),
    ];
    out.extend(checks.into_iter().filter_map(Result::err));
}

/// Checks one element on its own and returns the first rule it breaks.
pub fn validate_element(e: &Element) -> Result<(), ElementError> {
    let mut errors = Vec::new();
    collect_element_errors(e, &mut errors);
    errors.into_iter().next().map_or(Ok(()), Err)
}

/// Every rule `e` breaks on its own.
pub fn element_errors(e: &Element) -> Vec<ElementError> {
    let mut errors = Vec::new();
    collect_element_errors(e, &mut errors);
    errors
}

fn collect_element_errors(e: &Element, out: &mut Vec<ElementError>) {
    transform_errors(&e.transform, out);
    let mut check = |r: Result<(), ElementError>| {
        if let Err(err) = r {
            out.push(err);
        }
    };
    let s = &e.style;
    check(ranged("style.stroke_width", s.stroke_width, 0.0, MAX_STROKE_WIDTH));
    check(ranged("style.opacity", s.opacity, 0.0, 1.0));
    check(ranged("style.corner_radius", s.corner_radius, 0.0, MAX_CORNER_RADIUS));
    if e.index.as_str().len() > MAX_INDEX_LEN {
        check(Err(ElementError::IndexTooLong { len: e.index.as_str().len() }));
    }
    if e.group_ids.len() > MAX_GROUP_DEPTH {
        check(Err(ElementError::GroupDepth { len: e.group_ids.len(), max: MAX_GROUP_DEPTH }));
    }
    let mut seen = HashSet::new();
    for g in &e.group_ids {
        if !seen.insert(g) {
            check(Err(ElementError::DuplicateGroup(g.clone())));
        }
    }
    if e.frame_id.as_ref() == Some(&e.id) {
        check(Err(ElementError::SelfReference { field: "frame_id" }));
    }
    if let Some(name) = &e.name {
        check(text_ok("name", name));
    }
    if let Some(link) = &e.link
        && (link.len() > MAX_LINK_BYTES || link.chars().any(char::is_control))
    {
        check(Err(ElementError::TooLong { field: "link", len: link.len(), max: MAX_LINK_BYTES }));
    }
    if let Some(c) = &e.custom
        && c.len() > MAX_CUSTOM_KEYS
    {
        check(Err(ElementError::TooLong { field: "custom", len: c.len(), max: MAX_CUSTOM_KEYS }));
    }
    kind_errors(e, out);
}

fn kind_errors(e: &Element, out: &mut Vec<ElementError>) {
    match &e.kind {
        ElementKind::Rectangle | ElementKind::Diamond | ElementKind::Ellipse => {}
        ElementKind::Line(d) | ElementKind::Arrow(d) => {
            let is_arrow = matches!(e.kind, ElementKind::Arrow(_));
            let min_points = if d.closed { 3 } else { 2 };
            if d.points.len() < min_points {
                put(
                    out,
                    Err(ElementError::TooFewPoints { kind: e.tag().name(), min: min_points, have: d.points.len() }),
                );
            }
            points_errors(e, &d.points, out);
            if d.closed && is_arrow {
                put(out, Err(ElementError::NotApplicable("`closed` is only for lines; an arrow cannot be closed")));
            }
            if let Some(hs) = d.head_size {
                put(out, ranged("head_size", hs, 0.0, MAX_STROKE_WIDTH * 10.0));
            }
            linear_extras(e, d, out);
        }
        ElementKind::Freehand(d) => {
            if d.points.is_empty() {
                put(out, Err(ElementError::TooFewPoints { kind: "freehand", min: 1, have: 0 }));
            }
            points_errors(e, &d.points, out);
            if !d.pressures.is_empty() && d.pressures.len() != d.points.len() {
                put(out, Err(ElementError::PressureCount { pressures: d.pressures.len(), points: d.points.len() }));
            }
            if d.pressures.len() > MAX_POINTS {
                put(out, Err(ElementError::TooLong { field: "pressures", len: d.pressures.len(), max: MAX_POINTS }));
            } else {
                for p in &d.pressures {
                    put(out, ranged("pressures", *p, 0.0, 1.0));
                }
            }
        }
        ElementKind::Text(d) => {
            let len = d.content.chars().take(MAX_TEXT_CHARS + 1).count();
            if len > MAX_TEXT_CHARS {
                put(out, Err(ElementError::TooLong { field: "content", len, max: MAX_TEXT_CHARS }));
            }
            put(out, ranged("font.size", d.font.size, f64::MIN_POSITIVE, MAX_FONT_SIZE));
            put(out, ranged("font.line_height", d.font.line_height, f64::MIN_POSITIVE, 10.0));
            let mut prev_end = 0u32;
            for r in &d.runs {
                let ok = r.start < r.end && r.start >= prev_end && (r.end as usize) <= len;
                if !ok {
                    put(out, Err(ElementError::TextRun { start: r.start, end: r.end, len }));
                    break;
                }
                prev_end = r.end;
            }
            if d.container_id.as_ref() == Some(&e.id) {
                put(out, Err(ElementError::SelfReference { field: "container_id" }));
            }
            if let Some(lp) = d.label_position {
                put(out, ranged("label_position", lp, 0.0, 1.0));
            }
        }
        ElementKind::Math(d) => {
            let len = d.latex.chars().take(MAX_LATEX_CHARS + 1).count();
            if len > MAX_LATEX_CHARS {
                put(out, Err(ElementError::TooLong { field: "latex", len, max: MAX_LATEX_CHARS }));
            }
            put(out, ranged("font_size", d.font_size, f64::MIN_POSITIVE, MAX_FONT_SIZE));
        }
        ElementKind::Image(d) => {
            if let Some(c) = &d.crop {
                for (f, v) in [("crop.x", c.x), ("crop.y", c.y), ("crop.width", c.width), ("crop.height", c.height)] {
                    put(out, ranged(f, v, 0.0, MAX_COORD));
                }
                if c.width <= 0.0 || c.height <= 0.0 {
                    put(out, Err(ElementError::NotApplicable("a crop must have a width and height above 0")));
                }
            }
        }
        ElementKind::Frame(d) => {
            if let Some(n) = &d.name {
                put(out, text_ok("frame.name", n));
            }
            if e.transform.angle != 0.0 {
                put(out, Err(ElementError::NotApplicable("frames cannot be rotated")));
            }
            if e.frame_id.is_some() {
                put(out, Err(ElementError::NotApplicable("frames cannot be nested: a frame has no `frame_id`")));
            }
        }
    }
}

fn put(out: &mut Vec<ElementError>, r: Result<(), ElementError>) {
    if let Err(err) = r {
        out.push(err);
    }
}

fn points_errors(e: &Element, points: &[kurbo::Point], out: &mut Vec<ElementError>) {
    if points.len() > MAX_POINTS {
        out.push(ElementError::TooLong { field: "points", len: points.len(), max: MAX_POINTS });
        return;
    }
    // `from_points` skips unusable points, so they must be refused before it is asked for the bounds.
    if !points.iter().all(|p| drafftink_geom::point_is_finite(*p)) {
        out.push(ElementError::NotFinite { field: "points" });
        return;
    }
    let Some(bounds) = aabb::from_points(points.iter().copied()) else { return };
    let (min_x, min_y, max_x, max_y) = (bounds.x0, bounds.y0, bounds.x1, bounds.y1);
    let tol = |v: f64| BOX_TOLERANCE + v.abs() * 1.0e-9;
    if min_x.abs() > tol(max_x) || min_y.abs() > tol(max_y) {
        out.push(ElementError::PointsNotAtOrigin { x: min_x, y: min_y });
        return;
    }
    let (pw, ph) = (max_x - min_x, max_y - min_y);
    let t = &e.transform;
    if (t.w - pw).abs() > tol(pw) || (t.h - ph).abs() > tol(ph) {
        out.push(ElementError::BoxMismatch { w: t.w, h: t.h, pw, ph });
    }
}

fn linear_extras(e: &Element, d: &LinearData, out: &mut Vec<ElementError>) {
    for (field, b) in [("start_binding", &d.start_binding), ("end_binding", &d.end_binding)] {
        if let Some(b) = b {
            if b.target == e.id {
                out.push(ElementError::SelfReference { field });
            }
            for v in b.fixed_point {
                if let Err(err) = ranged("fixed_point", v, -MAX_FIXED_POINT, MAX_FIXED_POINT) {
                    out.push(err);
                }
            }
        }
    }
    let elbow = d.path == crate::element::PathType::Elbow;
    if !elbow && (!d.fixed_segments.is_empty() || d.start_is_special || d.end_is_special) {
        out.push(ElementError::NotApplicable("fixed segments and stubs only exist on elbow paths"));
    }
    let mut prev = 0u32;
    for s in &d.fixed_segments {
        let in_range = (s.index as usize) >= 1 && (s.index as usize) < d.points.len();
        if !in_range || s.index <= prev {
            out.push(ElementError::FixedSegment { index: s.index, max: d.points.len() });
            break;
        }
        prev = s.index;
        if ![s.start.x, s.start.y, s.end.x, s.end.y].iter().all(|v| v.is_finite()) {
            out.push(ElementError::NotFinite { field: "fixed_segments" });
            break;
        }
    }
}

/// How serious an [`Issue`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The data is inconsistent and must be repaired before use. A file with an error is refused.
    Error,
    /// The data is usable and a consumer resolves it on its own (a reference to a deleted element, a stale group).
    Warning,
}

/// A rule that involves more than one element, or the document.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DocError {
    /// An element breaks a rule on its own.
    #[error("{0}")]
    Element(#[from] ElementError),
    /// Two elements share an id.
    #[error("the id is used by more than one element")]
    DuplicateId,
    /// Z-order keys are not strictly increasing: a tie or an inversion. Run `zindex::repair`.
    #[error("z-order key {key} does not come after the previous key {previous}; repair the keys")]
    IndexOrder {
        /// Key of this element.
        key: ZIndex,
        /// Key of the element before it.
        previous: ZIndex,
    },
    /// `frame_id` names no element.
    #[error("frame {0} does not exist")]
    FrameMissing(ElementId),
    /// `frame_id` names an element that is not a frame.
    #[error("{id} is {}, not a frame, so it cannot be a frame_id", crate::words::Article::new(.kind))]
    NotAFrame {
        /// The element named.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
    },
    /// The frame is deleted but the child is not.
    #[error("frame {0} is deleted but this element is still its child")]
    FrameDeleted(ElementId),
    /// The child sits above its frame in z-order; children belong directly below the frame.
    #[error("this element sits above its frame {0}; children belong below the frame")]
    ChildAboveFrame(ElementId),
    /// `container_id` names no element.
    #[error("the container {0} of this label does not exist")]
    ContainerMissing(ElementId),
    /// The container cannot hold a label.
    #[error("{id} is {}, which cannot hold a label", crate::words::Article::new(.kind))]
    NotAContainer {
        /// The element named.
        id: ElementId,
        /// Its kind.
        kind: &'static str,
    },
    /// The container is deleted but its label is not.
    #[error("the container {0} is deleted but its label is not")]
    ContainerDeleted(ElementId),
    /// Two live texts claim the same container.
    #[error("{container} has more than one label ({other} is the other)")]
    TwoLabels {
        /// The container.
        container: ElementId,
        /// The other label.
        other: ElementId,
    },
    /// The label is below its container.
    #[error("this label sits below its container {0}; it belongs directly above it")]
    LabelBelowContainer(ElementId),
    /// A binding names no element.
    #[error("the {end} binding targets {target}, which does not exist")]
    BindingTargetMissing {
        /// `start` or `end`.
        end: &'static str,
        /// The target id.
        target: ElementId,
    },
    /// A binding targets a kind arrows cannot attach to.
    #[error(
        "the {end} binding targets {target}, {}; arrows attach to rectangles, diamonds, ellipses, text, images and frames",
        crate::words::Article::new(.kind)
    )]
    BindingTargetNotBindable {
        /// `start` or `end`.
        end: &'static str,
        /// The target id.
        target: ElementId,
        /// Its kind.
        kind: &'static str,
    },
    /// A binding targets a deleted element.
    #[error("the {end} binding targets {target}, which is deleted; treat the end as unbound")]
    BindingTargetDeleted {
        /// `start` or `end`.
        end: &'static str,
        /// The target id.
        target: ElementId,
    },
    /// Members of a group disagree about the groups around it.
    #[error(
        "group {0} is nested differently in different members; all members of a group must share the same outer groups"
    )]
    GroupStack(GroupId),
    /// A group with one live member counts as no group.
    #[error("group {0} has a single live member, so it counts as no group")]
    GroupSingleton(GroupId),
    /// Two live elements have the same name.
    #[error("the name {name:?} is also used by {other}; names must be unique so `@name` is unambiguous")]
    DuplicateName {
        /// The name.
        name: String,
        /// The other element.
        other: ElementId,
    },
    /// An image refers to a blob that is not in the files table.
    #[error("the picture {0} is not in the files table")]
    FileMissing(FileId),
    /// A blob's bytes do not hash to its id.
    #[error("the blob listed as {0} has different content; the id must be the SHA-256 of the bytes")]
    FileHashMismatch(FileId),
    /// A blob is too large.
    #[error("the blob {id} is {len} bytes, and the limit is {max}")]
    FileTooLarge {
        /// Blob id.
        id: FileId,
        /// Size.
        len: usize,
        /// Limit.
        max: usize,
    },
    /// A blob has a bad media type.
    #[error("the blob {id} has media type {mime:?}; use an image/* type like image/png")]
    BadMime {
        /// Blob id.
        id: FileId,
        /// The media type.
        mime: String,
    },
    /// Too many elements or blobs.
    #[error("the document has {len} {what}, and the limit is {max}")]
    TooMany {
        /// What is counted.
        what: &'static str,
        /// Count.
        len: usize,
        /// Limit.
        max: usize,
    },
    /// The document name is empty or too long.
    #[error("the document name must be 1 to {MAX_NAME_CHARS} characters without control characters")]
    BadDocName,
    /// The schema version is not one this build knows.
    #[error("schema version {0} is not supported by this build")]
    BadVersion(u32),
}

impl DocError {
    /// Errors must be repaired; warnings are resolved by the reader.
    pub fn severity(&self) -> Severity {
        match self {
            DocError::FrameDeleted(_)
            | DocError::ChildAboveFrame(_)
            | DocError::ContainerDeleted(_)
            | DocError::LabelBelowContainer(_)
            | DocError::BindingTargetDeleted { .. }
            | DocError::GroupSingleton(_)
            | DocError::DuplicateName { .. }
            | DocError::FileMissing(_) => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

/// One finding.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    /// How serious it is.
    pub severity: Severity,
    /// The element it is about, if any.
    pub element: Option<ElementId>,
    /// What is wrong.
    pub error: DocError,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.element {
            Some(id) => write!(f, "element {id}: {}", self.error),
            None => write!(f, "{}", self.error),
        }
    }
}

/// Everything [`validate_document`] found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    /// All findings, in element order.
    pub issues: Vec<Issue>,
}

impl Report {
    /// Findings that must be repaired.
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.severity == Severity::Error)
    }

    /// Findings the reader resolves on its own.
    pub fn warnings(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.severity == Severity::Warning)
    }

    /// True when at least one finding is an error.
    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    /// True when there is nothing at all to report.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    fn add(&mut self, element: Option<&ElementId>, error: DocError) {
        self.issues.push(Issue { severity: error.severity(), element: element.cloned(), error });
    }
}

/// How strictly z-order ties are treated.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    /// A file: keys must be strictly increasing.
    Strict,
    /// A live store: ties are legal (broken by id) and only reported as warnings.
    Tolerant,
}

/// Checks a whole document: every element, then the relations between them.
pub fn validate_document(doc: &Document) -> Report {
    let mut report = Report::default();
    if doc.schema_version == 0 || doc.schema_version > crate::document::SCHEMA_VERSION {
        report.add(None, DocError::BadVersion(doc.schema_version));
    }
    check_name(&doc.name, &mut report);
    let elements: Vec<&Element> = doc.elements.iter().collect();
    run(&elements, &doc.files.iter().collect::<Vec<_>>(), Order::Strict, &mut report);
    report
}

/// Checks a store's committed contents, with the same rules as [`validate_document`] except that z-order ties are
/// warnings (concurrent inserts make them).
pub fn validate_scene<R: SceneRead>(r: &R) -> Report {
    let mut report = Report::default();
    check_name(&r.meta().name, &mut report);
    let elements: Vec<&Element> = r.iter_all().collect();
    run(&elements, &r.files().collect::<Vec<_>>(), Order::Tolerant, &mut report);
    report
}

fn check_name(name: &str, report: &mut Report) {
    if !is_valid_name(name) {
        report.add(None, DocError::BadDocName);
    }
}

fn run(elements: &[&Element], files: &[(&FileId, &crate::document::FileEntry)], order: Order, report: &mut Report) {
    if elements.len() > MAX_ELEMENTS {
        report.add(None, DocError::TooMany { what: "elements", len: elements.len(), max: MAX_ELEMENTS });
        return;
    }
    if files.len() > MAX_FILES {
        report.add(None, DocError::TooMany { what: "files", len: files.len(), max: MAX_FILES });
        return;
    }
    let mut by_id: HashMap<&ElementId, &Element> = HashMap::with_capacity(elements.len());
    for e in elements {
        if by_id.insert(&e.id, e).is_some() {
            report.add(Some(&e.id), DocError::DuplicateId);
        }
    }

    let mut previous: Option<&Element> = None;
    for e in elements {
        for err in element_errors(e) {
            report.add(Some(&e.id), DocError::Element(err));
        }
        if let Some(p) = previous {
            let bad = if order == Order::Strict { e.index <= p.index } else { e.index < p.index };
            if bad {
                report.add(Some(&e.id), DocError::IndexOrder { key: e.index.clone(), previous: p.index.clone() });
            } else if order == Order::Tolerant && e.index == p.index {
                // A tie in a live store: reported as a warning-level order issue below.
                report.issues.push(Issue {
                    severity: Severity::Warning,
                    element: Some(e.id.clone()),
                    error: DocError::IndexOrder { key: e.index.clone(), previous: p.index.clone() },
                });
            }
        }
        previous = Some(e);
        if !e.deleted {
            relations(e, &by_id, report);
        }
    }

    check_labels(elements, &by_id, report);
    check_groups(elements, report);
    check_names(elements, report);
    check_files(elements, files, report);
}

fn relations(e: &Element, by_id: &HashMap<&ElementId, &Element>, report: &mut Report) {
    if let Some(fid) = &e.frame_id {
        match by_id.get(fid) {
            None => report.add(Some(&e.id), DocError::FrameMissing(fid.clone())),
            Some(f) if f.tag() != Kind::Frame => {
                report.add(Some(&e.id), DocError::NotAFrame { id: fid.clone(), kind: f.tag().name() });
            }
            Some(f) => {
                if f.deleted {
                    report.add(Some(&e.id), DocError::FrameDeleted(fid.clone()));
                } else if e.index > f.index {
                    report.add(Some(&e.id), DocError::ChildAboveFrame(fid.clone()));
                }
            }
        }
    }
    if let Some(d) = e.linear_data() {
        for (end, b) in [("start", &d.start_binding), ("end", &d.end_binding)] {
            let Some(b) = b else { continue };
            match by_id.get(&b.target) {
                None => report.add(Some(&e.id), DocError::BindingTargetMissing { end, target: b.target.clone() }),
                Some(t) if !t.tag().is_bindable() => report.add(
                    Some(&e.id),
                    DocError::BindingTargetNotBindable { end, target: b.target.clone(), kind: t.tag().name() },
                ),
                Some(t) if t.deleted => {
                    report.add(Some(&e.id), DocError::BindingTargetDeleted { end, target: b.target.clone() });
                }
                Some(_) => {}
            }
        }
    }
}

fn check_labels(elements: &[&Element], by_id: &HashMap<&ElementId, &Element>, report: &mut Report) {
    let mut label_of: HashMap<&ElementId, &ElementId> = HashMap::new();
    for e in elements.iter().filter(|e| !e.deleted) {
        let Some(cid) = e.container_id() else { continue };
        match by_id.get(cid) {
            None => report.add(Some(&e.id), DocError::ContainerMissing(cid.clone())),
            Some(c) if !c.tag().is_label_container() => {
                report.add(Some(&e.id), DocError::NotAContainer { id: cid.clone(), kind: c.tag().name() });
            }
            Some(c) => {
                if c.deleted {
                    report.add(Some(&e.id), DocError::ContainerDeleted(cid.clone()));
                    continue;
                }
                if e.index < c.index {
                    report.add(Some(&e.id), DocError::LabelBelowContainer(cid.clone()));
                }
                if let Some(other) = label_of.insert(cid, &e.id) {
                    report.add(Some(&e.id), DocError::TwoLabels { container: cid.clone(), other: other.clone() });
                }
            }
        }
    }
}

fn check_groups(elements: &[&Element], report: &mut Report) {
    // For each group: the groups around it (the stack after it) as seen on the first live member, and the count.
    let mut seen: HashMap<&GroupId, (&[GroupId], usize)> = HashMap::new();
    let mut reported: HashSet<&GroupId> = HashSet::new();
    for e in elements.iter().filter(|e| !e.deleted) {
        for (i, g) in e.group_ids.iter().enumerate() {
            let outer = e.group_ids.get(i + 1..).unwrap_or(&[]);
            match seen.get_mut(g) {
                None => {
                    seen.insert(g, (outer, 1));
                }
                Some((first, count)) => {
                    *count += 1;
                    if *first != outer && reported.insert(g) {
                        report.add(Some(&e.id), DocError::GroupStack(g.clone()));
                    }
                }
            }
        }
    }
    let mut singles: Vec<&GroupId> = seen.iter().filter(|(_, (_, n))| *n == 1).map(|(g, _)| *g).collect();
    singles.sort();
    for g in singles {
        report.add(None, DocError::GroupSingleton(g.clone()));
    }
}

fn check_names(elements: &[&Element], report: &mut Report) {
    let mut names: BTreeMap<&str, &ElementId> = BTreeMap::new();
    for e in elements.iter().filter(|e| !e.deleted) {
        if let Some(n) = &e.name
            && let Some(other) = names.insert(n.as_str(), &e.id)
        {
            report.add(Some(&e.id), DocError::DuplicateName { name: n.clone(), other: other.clone() });
        }
    }
}

fn check_files(elements: &[&Element], files: &[(&FileId, &crate::document::FileEntry)], report: &mut Report) {
    let present: HashSet<&FileId> = files.iter().map(|(id, _)| *id).collect();
    for e in elements.iter().filter(|e| !e.deleted) {
        if let ElementKind::Image(i) = &e.kind
            && let Some(f) = &i.file_id
            && !present.contains(f)
        {
            report.add(Some(&e.id), DocError::FileMissing(f.clone()));
        }
    }
    for (id, entry) in files {
        if !is_image_media_type(&entry.mime) {
            report.add(None, DocError::BadMime { id: (*id).clone(), mime: entry.mime.chars().take(80).collect() });
        }
        if entry.bytes.len() > MAX_FILE_BYTES {
            report.add(None, DocError::FileTooLarge { id: (*id).clone(), len: entry.bytes.len(), max: MAX_FILE_BYTES });
        } else if FileId::of(&entry.bytes) != **id {
            report.add(None, DocError::FileHashMismatch((*id).clone()));
        }
    }
}
