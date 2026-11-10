//! Turns an [`OldDoc`] into a native `Document`.
//!
//! Rules (`docs/model.md` section 10):
//! * Groups stop being elements. Each group with at least two non-empty members becomes a `GroupId`; its members get
//!   it after their own groups (innermost first). A group that adds no information (one non-empty child) gets none.
//! * A group's `rotation` is baked into its members: their boxes move about the group's pivot and their angles
//!   grow by the rotation. Lines and freehand strokes have no angle in the old model, so their points move.
//! * Text and formula boxes are measured by the injected [`TextMeasure`], because the old file did not store them.

use std::collections::{BTreeMap, HashSet};

use drafftink_scene::limits::{
    MAX_COORD, MAX_CORNER_RADIUS, MAX_ELEMENTS, MAX_FILE_BYTES, MAX_FILES, MAX_FONT_SIZE, MAX_GROUP_DEPTH, MAX_POINTS,
    MAX_STROKE_WIDTH,
};
use drafftink_scene::{
    Document, Element, ElementId, ElementKind, FileEntry, FileId, FreehandData, GroupId, Head, IdSource, ImageData,
    Kind, LinearData, MathData, PressureMode, Style, TextData, Transform, ZIndex,
};
use kurbo::{Point, Rect, Vec2};

use super::model::{Body, ImageFormat, OImage, OLinear, OShape, OStyle, OText, OldDoc, leaf_count};
use crate::error::{ImportReport, Imported, IoError};
use crate::measure::{EstimateMeasure, MathBox, TextMeasure};

/// Group rotations below this are not drawn by the old renderer, so they are not baked either.
const MIN_GROUP_ROTATION: f64 = 0.001;

pub(crate) fn build(
    old: &OldDoc,
    ids: &mut dyn IdSource,
    measure: &dyn TextMeasure,
    mut report: ImportReport,
) -> Result<Imported, IoError> {
    let mut b = Builder {
        ids,
        measure,
        report: &mut report,
        used_ids: HashSet::new(),
        used_groups: HashSet::new(),
        elements: Vec::new(),
        files: BTreeMap::new(),
        stack: Vec::new(),
        counter: 0,
        full: false,
    };
    for s in &old.shapes {
        b.emit(s);
    }
    let Builder { mut elements, files, full, .. } = b;
    if full {
        return Err(IoError::Limit { what: "document", max: format!("{MAX_ELEMENTS} elements") });
    }
    // Safety net: whatever the input, only elements that satisfy the model leave this function.
    let before = elements.len();
    elements.retain(|e| drafftink_scene::element_errors(e).is_empty());
    if elements.len() < before {
        report.warn(format!(
            "{} element(s) are outside the model's limits after conversion and were dropped",
            before - elements.len()
        ));
    }
    let keys = ZIndex::rebalance(elements.len()).map_err(|e| IoError::invalid("legacy document", e.to_string()))?;
    for (e, k) in elements.iter_mut().zip(keys) {
        e.index = k;
    }
    let mut doc = Document::new();
    if let Some(name) = old.name.as_deref().map(clean_name).filter(|n| !n.is_empty()) {
        doc.name = name;
    }
    doc.elements = elements;
    doc.files = files;
    Ok(Imported { document: doc, report })
}

fn clean_name(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(drafftink_scene::limits::MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

struct Builder<'a> {
    ids: &'a mut dyn IdSource,
    measure: &'a dyn TextMeasure,
    report: &'a mut ImportReport,
    used_ids: HashSet<String>,
    used_groups: HashSet<String>,
    elements: Vec<Element>,
    files: BTreeMap<FileId, FileEntry>,
    /// Groups around the shape being emitted, outermost first.
    stack: Vec<GroupId>,
    counter: u64,
    full: bool,
}

fn finite(v: f64) -> f64 {
    if v.is_finite() { v.clamp(-MAX_COORD, MAX_COORD) } else { 0.0 }
}

/// Coordinates of line and stroke points. Half the range, so that the extent of the points (a box size) stays
/// within the limit and a baked rotation cannot push them out.
fn point_coord(v: f64) -> f64 {
    if v.is_finite() { v.clamp(-MAX_COORD / 4.0, MAX_COORD / 4.0) } else { 0.0 }
}

fn font_size(v: f64) -> f64 {
    if v.is_finite() { v.clamp(1.0, MAX_FONT_SIZE) } else { 20.0 }
}

/// Seven-field style with the model's bounds applied.
fn style_of(o: &OStyle, ids: &mut dyn IdSource) -> Style {
    Style {
        stroke_color: o.stroke,
        stroke_width: o.stroke_width.clamp(0.0, MAX_STROKE_WIDTH),
        fill_color: o.fill,
        fill_pattern: o.fill_pattern,
        sloppiness: o.sloppiness,
        opacity: o.opacity.clamp(0.0, 1.0),
        seed: o.seed.unwrap_or_else(|| ids.seed()),
        ..Style::default()
    }
}

impl Builder<'_> {
    fn element_id(&mut self, old: Option<&str>) -> ElementId {
        if let Some(id) = old.and_then(|o| ElementId::new(o).ok())
            && self.used_ids.insert(id.as_str().to_owned())
        {
            return id;
        }
        for _ in 0..32 {
            let id = self.ids.element_id();
            if self.used_ids.insert(id.as_str().to_owned()) {
                return id;
            }
        }
        loop {
            self.counter += 1;
            if let Ok(id) = ElementId::new(&format!("mig{}", self.counter))
                && self.used_ids.insert(id.as_str().to_owned())
            {
                return id;
            }
        }
    }

    fn group_id(&mut self, old: Option<&str>) -> GroupId {
        if let Some(id) = old.and_then(|o| GroupId::new(o).ok())
            && self.used_groups.insert(id.as_str().to_owned())
        {
            return id;
        }
        for _ in 0..32 {
            let id = self.ids.group_id();
            if self.used_groups.insert(id.as_str().to_owned()) {
                return id;
            }
        }
        loop {
            self.counter += 1;
            if let Ok(id) = GroupId::new(&format!("mgrp{}", self.counter))
                && self.used_groups.insert(id.as_str().to_owned())
            {
                return id;
            }
        }
    }

    fn push(&mut self, mut e: Element, st: &OStyle) {
        if self.elements.len() >= MAX_ELEMENTS {
            self.full = true;
            return;
        }
        e.style = Style {
            corner_radius: e.style.corner_radius,
            stroke_style: e.style.stroke_style,
            ..style_of(st, self.ids)
        };
        e.group_ids = self.stack.iter().rev().cloned().collect();
        self.elements.push(e);
    }

    fn emit(&mut self, s: &OShape) {
        if self.full {
            return;
        }
        match &s.body {
            Body::Group { children, rotation } => self.group(s, children, *rotation),
            Body::Rect { pos, w, h, radius, rotation } => {
                let id = self.element_id(s.id.as_deref());
                let mut e = Element::new(id, ElementKind::Rectangle, ZIndex::first());
                e.transform = boxed(pos.x, pos.y, *w, *h, *rotation);
                e.style.corner_radius = radius.clamp(0.0, MAX_CORNER_RADIUS);
                self.push(e, &s.style);
            }
            Body::Ellipse { center, rx, ry, rotation } => {
                let id = self.element_id(s.id.as_deref());
                let (rx, ry) = (finite(rx.abs()), finite(ry.abs()));
                let mut e = Element::new(id, ElementKind::Ellipse, ZIndex::first());
                e.transform = boxed(center.x - rx, center.y - ry, 2.0 * rx, 2.0 * ry, *rotation);
                self.push(e, &s.style);
            }
            Body::Line(l) => self.linear(s, l, None),
            Body::Arrow(l, head) => self.linear(s, l, Some(*head)),
            Body::Freehand { points, pressures, closed } => self.freehand(s, points, pressures, *closed),
            Body::Text(t) => self.text(s, t),
            Body::Math { pos, latex, size, rotation } => self.math(s, *pos, latex, *size, *rotation),
            Body::Image(i) => self.image(s, i),
        }
    }

    fn group(&mut self, s: &OShape, children: &[OShape], rotation: f64) {
        let non_empty = children.iter().filter(|c| leaf_count(c) > 0).count();
        let wants_id = non_empty >= 2;
        let mut pushed = false;
        if wants_id {
            if self.stack.len() < MAX_GROUP_DEPTH {
                let gid = self.group_id(s.id.as_deref());
                self.stack.push(gid);
                pushed = true;
            } else {
                self.report.warn("groups are nested deeper than the model allows; the innermost levels are merged");
            }
        }
        let start = self.elements.len();
        for c in children {
            self.emit(c);
        }
        if pushed {
            self.stack.pop();
        }
        if rotation.is_finite() && rotation.abs() > MIN_GROUP_ROTATION && start < self.elements.len() {
            let pivot = self.old_bounds_group(children).center();
            if let Some(slice) = self.elements.get_mut(start..) {
                bake_rotation(slice, pivot, rotation);
            }
        }
    }

    fn linear(&mut self, s: &OShape, l: &OLinear, head: Option<f64>) {
        let mut pts: Vec<Point> = l.points.iter().map(|p| Point::new(point_coord(p.x), point_coord(p.y))).collect();
        if pts.len() > MAX_POINTS {
            self.report.warn(format!("a line has {} points; only the first {MAX_POINTS} are kept", pts.len()));
            pts.truncate(MAX_POINTS);
        }
        let kind = if head.is_some() { Kind::Arrow } else { Kind::Line };
        let id = self.element_id(s.id.as_deref());
        let Some(mut e) = Element::linear(id, kind, ZIndex::first(), &pts) else { return };
        let closed = l.closed && pts.len() >= 3 && head.is_none();
        let mut data = e.linear_data().cloned().unwrap_or_else(|| LinearData::new(Vec::new()));
        data.path = l.path;
        data.closed = closed;
        if let Some(h) = head {
            data.end_head = Head::Arrow;
            data.head_size = Some(finite(h).clamp(0.0, MAX_STROKE_WIDTH * 10.0));
        }
        e.kind = if head.is_some() { ElementKind::Arrow(data) } else { ElementKind::Line(data) };
        e.style.stroke_style = l.stroke_style;
        self.push(e, &s.style);
    }

    fn freehand(&mut self, s: &OShape, points: &[Point], pressures: &[f64], closed: bool) {
        if points.is_empty() {
            self.report.warn("a freehand stroke has no points; skipped");
            return;
        }
        let n = points.len().min(MAX_POINTS);
        if n < points.len() {
            self.report.warn(format!("a stroke has {} points; only the first {MAX_POINTS} are kept", points.len()));
        }
        let mut d =
            FreehandData::new(points.iter().take(n).map(|p| Point::new(point_coord(p.x), point_coord(p.y))).collect());
        if pressures.len() == n {
            d.pressures = pressures.iter().map(|p| if p.is_finite() { p.clamp(0.0, 1.0) } else { 1.0 }).collect();
            d.pressure_mode = PressureMode::Device;
        } else if !pressures.is_empty() {
            self.report.warn("a stroke has a different number of pressures than points; the pressures are dropped");
        }
        d.closed = closed;
        let id = self.element_id(s.id.as_deref());
        let mut e = Element::new(id, ElementKind::Freehand(d), ZIndex::first());
        if e.refit_points() {
            self.push(e, &s.style);
        }
    }

    fn text(&mut self, s: &OShape, t: &OText) {
        let mut data = TextData::new(t.content.clone());
        data.font.size = font_size(t.size);
        data.font.family = t.family;
        data.font.weight = t.weight;
        let chars = data.char_count();
        data.runs = TextData::runs_from_char_colors(&t.char_colors, chars);
        let (w, h) = self.text_box(&data);
        let id = self.element_id(s.id.as_deref());
        let mut e = Element::new(id, ElementKind::Text(data), ZIndex::first());
        e.transform = boxed(t.pos.x, t.pos.y, w, h, t.rotation);
        self.push(e, &s.style);
    }

    fn text_box(&self, t: &TextData) -> (f64, f64) {
        let (w, h) = self.measure.text_size(&t.content, t.font.family, t.font.weight, t.font.size, t.font.line_height);
        (finite(w).max(0.0), finite(h).max(0.0))
    }

    fn math_box(&self, latex: &str, size: f64) -> MathBox {
        let b = self.measure.math_size(latex, size).or_else(|| EstimateMeasure.math_size(latex, size));
        let b = b.unwrap_or(MathBox { width: 0.0, height: size, depth: 0.0 });
        MathBox { width: finite(b.width).max(0.0), height: finite(b.height).max(0.0), depth: finite(b.depth).max(0.0) }
    }

    fn math(&mut self, s: &OShape, pos: Point, latex: &str, size: f64, rotation: f64) {
        let size = font_size(size);
        let mut data = MathData::new(latex);
        data.font_size = size;
        let m = self.math_box(latex, size);
        // The old position is the baseline origin. The box top is `height` above it.
        let id = self.element_id(s.id.as_deref());
        let mut e = Element::new(id, ElementKind::Math(data), ZIndex::first());
        e.transform = boxed(pos.x, pos.y - m.height, m.width, m.height + m.depth, rotation);
        self.push(e, &s.style);
    }

    fn image(&mut self, s: &OShape, i: &OImage) {
        let mut file_id = None;
        if !i.data.is_empty() {
            if i.data.len() > MAX_FILE_BYTES {
                self.report.warn("an image is larger than the file limit; the picture is dropped");
            } else {
                let (id, entry) = FileEntry::new(sniff_mime(&i.data, i.format), i.data.clone());
                if self.files.len() < MAX_FILES || self.files.contains_key(&id) {
                    self.files.entry(id.clone()).or_insert(entry);
                    file_id = Some(id);
                } else {
                    self.report.warn("too many distinct images; some pictures are dropped");
                }
            }
        }
        let data = ImageData { file_id, natural_width: i.source_w, natural_height: i.source_h, crop: None };
        let id = self.element_id(s.id.as_deref());
        let mut e = Element::new(id, ElementKind::Image(data), ZIndex::first());
        e.transform = boxed(i.pos.x, i.pos.y, i.w, i.h, i.rotation);
        self.push(e, &s.style);
    }

    /// The old `Shape::bounds()` of a shape, which is what the old renderer used as the pivot of a rotation.
    fn old_bounds(&self, s: &OShape) -> Rect {
        match &s.body {
            Body::Rect { pos, w, h, .. } => Rect::new(pos.x, pos.y, pos.x + w, pos.y + h),
            Body::Ellipse { center, rx, ry, .. } => {
                Rect::new(center.x - rx, center.y - ry, center.x + rx, center.y + ry)
            }
            Body::Line(l) => points_rect(&l.points),
            Body::Arrow(l, head) => arrow_rect(&l.points, *head),
            Body::Freehand { points, .. } => points_rect(points),
            Body::Text(t) => {
                let mut d = TextData::new(t.content.clone());
                d.font.size = font_size(t.size);
                d.font.family = t.family;
                d.font.weight = t.weight;
                let (w, h) = self.text_box(&d);
                Rect::new(t.pos.x, t.pos.y, t.pos.x + w, t.pos.y + h)
            }
            Body::Math { pos, latex, size, .. } => {
                let m = self.math_box(latex, font_size(*size));
                Rect::new(pos.x, pos.y - m.height, pos.x + m.width, pos.y + m.depth)
            }
            Body::Image(i) => Rect::new(i.pos.x, i.pos.y, i.pos.x + i.w, i.pos.y + i.h),
            Body::Group { children, .. } => self.old_bounds_group(children),
        }
    }

    fn old_bounds_group(&self, children: &[OShape]) -> Rect {
        let mut it = children.iter().map(|c| self.old_bounds(c).abs());
        match it.next() {
            Some(first) => it.fold(first, |a, b| a.union(b)),
            None => Rect::ZERO,
        }
    }
}

fn boxed(x: f64, y: f64, w: f64, h: f64, angle: f64) -> Transform {
    let (x, y, w, h) = (finite(x), finite(y), finite(w), finite(h));
    Transform {
        x: x.min(x + w).clamp(-MAX_COORD, MAX_COORD),
        y: y.min(y + h).clamp(-MAX_COORD, MAX_COORD),
        w: w.abs().min(MAX_COORD),
        h: h.abs().min(MAX_COORD),
        angle: if angle.is_finite() { angle } else { 0.0 },
        flip_x: false,
        flip_y: false,
    }
}

fn points_rect(points: &[Point]) -> Rect {
    match drafftink_scene::geom::points_bounds(points) {
        Some((a, b, c, d)) => Rect::new(a, b, c, d),
        None => Rect::ZERO,
    }
}

/// The old arrow bounds: all points plus the two barbs of the head, which points from the first to the last point.
pub(crate) fn arrow_rect(points: &[Point], head_size: f64) -> Rect {
    let (Some(&start), Some(&end)) = (points.first(), points.last()) else { return Rect::ZERO };
    let d = end - start;
    let len = d.hypot();
    let dir = if len < f64::EPSILON { Vec2::new(1.0, 0.0) } else { d / len };
    let perp = Vec2::new(-dir.y, dir.x);
    let back = end - dir * head_size;
    let (left, right) = (back + perp * head_size * 0.5, back - perp * head_size * 0.5);
    points
        .iter()
        .fold(Rect::new(left.x.min(right.x), left.y.min(right.y), left.x.max(right.x), left.y.max(right.y)), |r, p| {
            Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y))
        })
}

fn sniff_mime(data: &[u8], format: Option<ImageFormat>) -> &'static str {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if data.len() >= 12 && data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else {
        match format {
            Some(ImageFormat::Jpeg) => "image/jpeg",
            Some(ImageFormat::WebP) => "image/webp",
            _ => "image/png",
        }
    }
}

/// Rotates every element about `pivot` by `theta` (clockwise on screen, radians).
fn bake_rotation(elements: &mut [Element], pivot: Point, theta: f64) {
    let (sin, cos) = theta.sin_cos();
    let rot = |p: Point| -> Point {
        let v = p - pivot;
        pivot + Vec2::new(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
    };
    for e in elements {
        let is_points = matches!(e.kind, ElementKind::Line(_) | ElementKind::Arrow(_) | ElementKind::Freehand(_));
        if is_points {
            let t = e.transform;
            let world: Vec<Point> = e.points().unwrap_or(&[]).iter().map(|p| rot(t.local_to_world(*p))).collect();
            match &mut e.kind {
                ElementKind::Line(d) | ElementKind::Arrow(d) => d.points = world,
                ElementKind::Freehand(d) => d.points = world,
                _ => {}
            }
            e.transform = Transform::new(0.0, 0.0, 0.0, 0.0);
            e.refit_points();
        } else {
            let c = rot(e.transform.center());
            e.transform.x = c.x - e.transform.w / 2.0;
            e.transform.y = c.y - e.transform.h / 2.0;
            e.transform.angle = (e.transform.angle + theta).rem_euclid(std::f64::consts::TAU);
        }
    }
}
