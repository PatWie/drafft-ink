//! Recomputes the facts of the legacy fixtures with the new code and compares them with the `.facts.json` files
//! that the old app's code wrote (`tests/fixtures/README.md`). This is the library behind
//! `cargo xtask fixtures --check`.
//!
//! What is compared, per document, exactly unless a tolerance is named:
//! * name; the number of elements per kind (old groups are not elements, so they are not counted);
//! * element ids and z-order: the new elements, back to front, are the old leaf shapes in pre-order;
//! * group membership: the old groups with two or more non-empty children are exactly the new groups, as sets of
//!   member ids, and every element lists its groups innermost first;
//! * rotation (modulo 2 pi when a rotated group was baked in), text sizes and fonts, image data, point counts;
//! * old-style bounds of every leaf (and of every group, as the union of its leaves) within 1e-6. A rotated
//!   group's baked rotation is undone first, about the old group's pivot. Formula bounds are skipped: the old
//!   estimate used the depth with the opposite sign (documented in `measure.rs`).
//! * the legacy CRDT snapshots decode to the dump stored next to them, and migrate to the same document as the
//!   JSON they were built from, apart from the two things the old CRDT lost (`closed` flags, group rotation).
//!
//! Not compared: hit-test and rectangle probes (they pin the old hit-test code, which does not exist any more) and
//! the CRDT round-trip table.
//!
//! The functions that take bytes are pure. [`check_fixture_facts`] reads a directory and is not available on wasm.

use std::collections::{BTreeMap, BTreeSet};

use drafftink_geom::aabb;
use drafftink_scene::{Document, Element, ElementKind, FontFamily, Kind, SeqIds};
use kurbo::{Point, Rect, Vec2};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::legacy;
use crate::measure::{EstimateMeasure, TextMeasure};

/// Absolute tolerance for coordinates (`tests/fixtures/README.md`: 1e-6).
pub const TOLERANCE: f64 = 1e-6;
const ROTATION_EPS: f64 = 1e-3;

/// The result for one fixture.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FixtureReport {
    /// Fixture name, for example `all_shapes` or `crdt/images`.
    pub name: String,
    /// Number of individual comparisons made.
    pub checks: usize,
    /// Comparisons that failed, one plain sentence each (capped).
    pub failures: Vec<String>,
    /// Things that were deliberately not compared, and why.
    pub skipped: Vec<String>,
}

impl FixtureReport {
    fn new(name: &str) -> Self {
        Self { name: name.to_owned(), ..Self::default() }
    }

    fn check(&mut self, ok: bool, what: impl FnOnce() -> String) {
        self.checks += 1;
        if !ok && self.failures.len() < 40 {
            self.failures.push(what());
        }
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.checks += 1;
        if self.failures.len() < 40 {
            self.failures.push(message.into());
        }
    }

    fn skip(&mut self, message: impl Into<String>) {
        let m = message.into();
        if !self.skipped.contains(&m) {
            self.skipped.push(m);
        }
    }
}

/// The result for a fixture directory.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// One entry per `.facts.json` file.
    pub fixtures: Vec<FixtureReport>,
    /// Files whose bytes differ from `MANIFEST.json`, are missing, or other problems with the directory itself.
    pub manifest_failures: Vec<String>,
}

impl Report {
    /// True when nothing failed and at least one fixture was checked.
    pub fn is_ok(&self) -> bool {
        !self.fixtures.is_empty()
            && self.manifest_failures.is_empty()
            && self.fixtures.iter().all(|f| f.failures.is_empty())
    }

    /// Number of failed comparisons, including manifest problems.
    pub fn failure_count(&self) -> usize {
        self.manifest_failures.len() + self.fixtures.iter().map(|f| f.failures.len()).sum::<usize>()
    }

    /// Total comparisons made.
    pub fn check_count(&self) -> usize {
        self.fixtures.iter().map(|f| f.checks).sum()
    }

    /// A multi-line text for a terminal.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for f in &self.fixtures {
            let status = if f.failures.is_empty() { "ok  " } else { "FAIL" };
            out.push_str(&format!("{status} {} ({} checks)\n", f.name, f.checks));
            for m in &f.failures {
                out.push_str(&format!("       - {m}\n"));
            }
        }
        for m in &self.manifest_failures {
            out.push_str(&format!("FAIL manifest: {m}\n"));
        }
        out.push_str(&format!(
            "{} fixtures, {} checks, {} failures\n",
            self.fixtures.len(),
            self.check_count(),
            self.failure_count()
        ));
        out
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Old facts
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Fact {
    idx: usize,
    id: String,
    ty: String,
    parent: Option<usize>,
    bounds: Rect,
    estimated: bool,
    rotation: f64,
    extra: Value,
}

fn rect_of(v: &Value) -> Option<Rect> {
    let a = v.as_array()?;
    let n = |i: usize| a.get(i).and_then(Value::as_f64);
    Some(Rect::new(n(0)?, n(1)?, n(2)?, n(3)?).abs())
}

fn parse_facts(doc: &Value) -> Option<Vec<Fact>> {
    doc.get("shapes")?
        .as_array()?
        .iter()
        .map(|s| {
            Some(Fact {
                idx: usize::try_from(s.get("idx")?.as_u64()?).ok()?,
                id: s.get("id")?.as_str()?.to_owned(),
                ty: s.get("type")?.as_str()?.to_owned(),
                parent: s.get("parent").and_then(Value::as_u64).and_then(|p| usize::try_from(p).ok()),
                bounds: rect_of(s.get("bounds")?)?,
                estimated: s.get("bounds_estimated").and_then(Value::as_bool).unwrap_or(false),
                rotation: s.get("rotation").and_then(Value::as_f64).unwrap_or(0.0),
                extra: s.get("extra").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn kind_of(ty: &str) -> Option<Kind> {
    Some(match ty {
        "Rectangle" => Kind::Rectangle,
        "Ellipse" => Kind::Ellipse,
        "Line" => Kind::Line,
        "Arrow" => Kind::Arrow,
        "Freehand" => Kind::Freehand,
        "Text" => Kind::Text,
        "Math" => Kind::Math,
        "Image" => Kind::Image,
        _ => return None,
    })
}

fn rect_close(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() <= TOLERANCE
        && (a.y0 - b.y0).abs() <= TOLERANCE
        && (a.x1 - b.x1).abs() <= TOLERANCE
        && (a.y1 - b.y1).abs() <= TOLERANCE
}

fn angle_close(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(std::f64::consts::TAU);
    d.min(std::f64::consts::TAU - d) <= 1e-9
}

// ---------------------------------------------------------------------------------------------------------------
// Old-style geometry of a new element
// ---------------------------------------------------------------------------------------------------------------

/// One baked group rotation: the pivot is the centre of the old group's bounds.
#[derive(Clone, Copy)]
struct Bake {
    theta: f64,
    pivot: Point,
}

fn unrotate(p: Point, b: Bake) -> Point {
    let (s, c) = (-b.theta).sin_cos();
    let v = p - b.pivot;
    b.pivot + Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// The bounds the old code reported for this shape: no stroke inflation, unrotated box for boxes, points (and the
/// arrow head) for lines. `bakes` lists the group rotations that were baked in, innermost first.
fn old_style_bounds(e: &Element, bakes: &[Bake]) -> Option<Rect> {
    match &e.kind {
        ElementKind::Line(_) | ElementKind::Arrow(_) | ElementKind::Freehand(_) => {
            let t = e.transform;
            let mut pts: Vec<Point> = e.points()?.iter().map(|p| t.local_to_world(*p)).collect();
            for b in bakes.iter().rev() {
                for p in &mut pts {
                    *p = unrotate(*p, *b);
                }
            }
            match &e.kind {
                ElementKind::Arrow(d) => Some(legacy_arrow_rect(&pts, d.head_size.unwrap_or(15.0))),
                _ => aabb::from_points(pts.iter().copied()),
            }
        }
        ElementKind::Math(_) | ElementKind::Frame(_) | ElementKind::Diamond => None,
        _ => {
            let mut c = e.transform.center();
            for b in bakes.iter().rev() {
                c = unrotate(c, *b);
            }
            let (hw, hh) = (e.transform.w / 2.0, e.transform.h / 2.0);
            Some(Rect::new(c.x - hw, c.y - hh, c.x + hw, c.y + hh))
        }
    }
}

fn legacy_arrow_rect(points: &[Point], head: f64) -> Rect {
    crate::legacy::arrow_bounds(points, head)
}

// ---------------------------------------------------------------------------------------------------------------
// Document facts
// ---------------------------------------------------------------------------------------------------------------

/// Checks one legacy document against its facts. `doc_bytes` is the uncompressed `CanvasDocument` JSON and
/// `facts_json` the uncompressed facts file. Uses `measure` for text sizes; with [`EstimateMeasure`] the estimated
/// text bounds are compared too.
pub fn check_legacy_facts(name: &str, doc_bytes: &[u8], facts_json: &[u8], measure: &dyn TextMeasure) -> FixtureReport {
    let mut r = FixtureReport::new(name);
    let Ok(facts_root) = serde_json::from_slice::<Value>(facts_json) else {
        r.fail("the facts file is not JSON");
        return r;
    };
    if let Some(sha) = facts_root.get("source_sha256").and_then(Value::as_str) {
        let actual = hex(&Sha256::digest(doc_bytes));
        r.check(actual == sha, || format!("the document bytes hash to {actual}, the facts say {sha}"));
    }
    let Some(facts) = facts_root.get("doc") else {
        r.fail("the facts file has no `doc`");
        return r;
    };
    let text = match std::str::from_utf8(doc_bytes) {
        Ok(t) => t,
        Err(_) => {
            r.fail("the document is not UTF-8");
            return r;
        }
    };
    let imported = match legacy::migrate_json(text, &mut SeqIds::new(), measure) {
        Ok(i) => i,
        Err(e) => {
            r.fail(format!("migration failed: {e}"));
            return r;
        }
    };
    let source: Option<Value> = serde_json::from_str(text).ok();
    compare(&mut r, &imported.document, facts, source.as_ref(), measure_is_estimate(measure));
    r
}

fn measure_is_estimate(m: &dyn TextMeasure) -> bool {
    // The estimate is the only measure whose result equals the old estimate. A probe tells without a downcast.
    let probe = m.text_size("abcd", FontFamily::GelPen, drafftink_scene::FontWeight::Regular, 10.0, 1.25);
    let est = EstimateMeasure.text_size("abcd", FontFamily::GelPen, drafftink_scene::FontWeight::Regular, 10.0, 1.25);
    (probe.0 - est.0).abs() < 1e-12 && (probe.1 - est.1).abs() < 1e-12
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn compare(r: &mut FixtureReport, doc: &Document, facts: &Value, source: Option<&Value>, estimate_exact: bool) {
    let Some(shapes) = parse_facts(facts) else {
        r.fail("the facts `shapes` list is malformed");
        return;
    };
    if let Some(n) = facts.get("name").and_then(Value::as_str) {
        r.check(doc.name == n, || format!("name is {:?}, expected {n:?}", doc.name));
    }
    let leaves: Vec<&Fact> = shapes.iter().filter(|s| s.ty != "Group").collect();
    let by_idx: BTreeMap<usize, &Fact> = shapes.iter().map(|s| (s.idx, s)).collect();
    let live: Vec<&Element> = doc.elements.iter().filter(|e| !e.deleted).collect();

    // Counts per kind.
    let mut expected: BTreeMap<Kind, usize> = BTreeMap::new();
    for l in &leaves {
        if let Some(k) = kind_of(&l.ty) {
            *expected.entry(k).or_default() += 1;
        }
    }
    let mut actual: BTreeMap<Kind, usize> = BTreeMap::new();
    for e in &live {
        *actual.entry(e.tag()).or_default() += 1;
    }
    r.check(expected == actual, || format!("element counts are {actual:?}, expected {expected:?}"));
    if let Some(total) = facts.get("shape_count_total").and_then(Value::as_u64) {
        let groups = shapes.len() - leaves.len();
        r.check(usize::try_from(total).ok() == Some(shapes.len()), || {
            format!("facts total {total} != listed shapes {}", shapes.len())
        });
        r.check(live.len() == shapes.len() - groups, || {
            format!("{} elements, expected {}", live.len(), shapes.len() - groups)
        });
    }

    // Ids and z-order.
    if live.len() == leaves.len() {
        let same = live.iter().zip(&leaves).all(|(e, l)| e.id.as_str() == l.id);
        r.check(same, || {
            let at = live.iter().zip(&leaves).position(|(e, l)| e.id.as_str() != l.id).unwrap_or(0);
            format!("z-order or ids differ from the old pre-order at position {at}")
        });
    } else {
        r.fail("cannot compare z-order: the element count differs");
        return;
    }
    let ordered = doc.elements.windows(2).all(|w| w[0].index < w[1].index);
    r.check(ordered, || "z-order keys are not strictly increasing".to_owned());

    // Groups.
    let children_of =
        |g: usize| -> Vec<usize> { shapes.iter().filter(|s| s.parent == Some(g)).map(|s| s.idx).collect() };
    let leaves_of = |g: usize| -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![g];
        while let Some(x) = stack.pop() {
            for c in children_of(x) {
                if by_idx.get(&c).is_some_and(|f| f.ty == "Group") {
                    stack.push(c);
                } else {
                    out.push(c);
                }
            }
        }
        out.sort_unstable();
        out
    };
    let mut expected_groups: BTreeSet<Vec<String>> = BTreeSet::new();
    let mut group_leaves: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for g in shapes.iter().filter(|s| s.ty == "Group") {
        let ls = leaves_of(g.idx);
        let non_empty = children_of(g.idx)
            .into_iter()
            .filter(|c| by_idx.get(c).is_some_and(|f| f.ty != "Group" || !leaves_of(*c).is_empty()))
            .count();
        if non_empty >= 2 {
            expected_groups.insert(ls.iter().filter_map(|i| by_idx.get(i)).map(|f| f.id.clone()).collect());
        }
        group_leaves.insert(g.idx, ls);
    }
    let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &live {
        for g in &e.group_ids {
            members.entry(g.to_string()).or_default().push(e.id.to_string());
        }
    }
    // Members are listed in z-order, which is the old leaf order, so they compare as ordered lists.
    let actual_groups: BTreeSet<Vec<String>> = members.values().cloned().collect();
    r.check(actual_groups == expected_groups, || {
        format!("group membership differs: {} groups, expected {}", actual_groups.len(), expected_groups.len())
    });
    let mut size_of: BTreeMap<String, usize> = BTreeMap::new();
    for (g, m) in &members {
        size_of.insert(g.clone(), m.len());
    }
    for e in &live {
        let sizes: Vec<usize> = e.group_ids.iter().map(|g| size_of.get(g.as_str()).copied().unwrap_or(0)).collect();
        r.check(sizes.windows(2).all(|w| w[0] < w[1]), || {
            format!("{}: groups are not innermost first: {sizes:?}", e.id)
        });
    }

    // Per-leaf details.
    for (e, l) in live.iter().zip(&leaves) {
        leaf_details(r, e, l, source, facts);
    }

    // Rotation chains, bounds.
    for (e, l) in live.iter().zip(&leaves) {
        let mut chain: Vec<Bake> = Vec::new();
        let mut has_math_pivot = false;
        let mut p = l.parent;
        while let Some(g) = p.and_then(|g| by_idx.get(&g)) {
            if g.rotation.abs() > ROTATION_EPS {
                chain.push(Bake { theta: g.rotation, pivot: g.bounds.center() });
                has_math_pivot |= group_leaves
                    .get(&g.idx)
                    .is_some_and(|ls| ls.iter().any(|i| by_idx.get(i).is_some_and(|f| f.ty == "Math")));
            }
            p = g.parent;
        }
        let total_rotation: f64 = l.rotation + chain.iter().map(|b| b.theta).sum::<f64>();
        if matches!(e.tag(), Kind::Rectangle | Kind::Ellipse | Kind::Text | Kind::Math | Kind::Image) {
            // Not `==` even without a baked group: the old serde_json parsed some floats one ulp off (`welcome.json`).
            let ok = angle_close(e.transform.angle, total_rotation);
            r.check(ok, || {
                format!("{}: angle {} but the old rotation chain gives {total_rotation}", e.id, e.transform.angle)
            });
        } else {
            r.check(e.transform.angle == 0.0, || format!("{}: a line or stroke has angle {}", e.id, e.transform.angle));
        }
        let comparable = l.ty != "Math" && !has_math_pivot && (!l.estimated || estimate_exact);
        if !comparable {
            r.skip("bounds of formulas, of anything under a rotated group that holds a formula, and (with a real text measure) of text");
            continue;
        }
        match old_style_bounds(e, &chain) {
            Some(b) => r.check(rect_close(b, l.bounds), || format!("{}: bounds {b:?}, old {:?}", e.id, l.bounds)),
            None => r.fail(format!("{}: no bounds for this kind", e.id)),
        }
    }

    // Group bounds as the union of their leaves' old-style bounds.
    for g in shapes.iter().filter(|s| s.ty == "Group") {
        let ls = &group_leaves[&g.idx];
        let has_empty_group = shapes
            .iter()
            .any(|s| s.ty == "Group" && s.parent == Some(g.idx) && group_leaves.get(&s.idx).is_some_and(Vec::is_empty));
        let has_math = ls.iter().any(|i| by_idx.get(i).is_some_and(|f| f.ty == "Math"));
        if ls.is_empty() || has_empty_group || has_math || (g.estimated && !estimate_exact) {
            r.skip("bounds of groups that are empty, hold an empty group or a formula, or (with a real measure) text");
            continue;
        }
        let mut u: Option<Rect> = None;
        for i in ls {
            // The old group bounds ignore every group rotation, so undo the chain of this leaf below `g`.
            let Some(pos) = leaves.iter().position(|l| l.idx == *i) else { continue };
            let Some(e) = live.get(pos) else { continue };
            let mut chain = Vec::new();
            let mut p = leaves.get(pos).and_then(|l| l.parent);
            while let Some(q) = p.and_then(|q| by_idx.get(&q)) {
                if q.rotation.abs() > ROTATION_EPS {
                    chain.push(Bake { theta: q.rotation, pivot: q.bounds.center() });
                }
                p = q.parent;
            }
            if let Some(b) = old_style_bounds(e, &chain) {
                u = Some(u.map_or(b, |x| x.union(b)));
            }
        }
        match u {
            Some(u) => {
                r.check(rect_close(u, g.bounds), || format!("group {}: bounds {u:?}, old {:?}", g.idx, g.bounds))
            }
            None => r.fail(format!("group {}: no bounds", g.idx)),
        }
    }
}

fn leaf_details(r: &mut FixtureReport, e: &Element, l: &Fact, source: Option<&Value>, _facts: &Value) {
    let x = &l.extra;
    match &e.kind {
        ElementKind::Text(t) => {
            r.check(x.get("chars").and_then(Value::as_u64) == Some(t.char_count() as u64), || {
                format!("{}: character count", e.id)
            });
            if let Some(f) = x.get("font_family").and_then(Value::as_str) {
                r.check(f == t.font.family.display_name(), || format!("{}: font {:?}, old {f:?}", e.id, t.font.family));
            }
            if let Some(s) = x.get("font_size").and_then(Value::as_f64) {
                r.check((t.font.size - s.max(1.0)).abs() <= TOLERANCE, || {
                    format!("{}: font size {}, old {s}", e.id, t.font.size)
                });
            }
            // Colours: count the coloured characters among the first `chars` entries of the stored array.
            if let Some(src) = source.and_then(|s| old_shape_by_id(s, &l.id))
                && let Some(arr) = src.get("char_colors").and_then(Value::as_array)
            {
                let want = arr.iter().take(t.char_count()).filter(|c| !c.is_null()).count();
                let got = t.char_colors().iter().filter(|c| c.is_some()).count();
                r.check(want == got, || format!("{}: {got} coloured characters, expected {want}", e.id));
            }
        }
        ElementKind::Math(m) => {
            if let Some(s) = x.get("latex").and_then(Value::as_str) {
                r.check(m.latex == s, || format!("{}: latex differs", e.id));
            }
            if let Some(s) = x.get("font_size").and_then(Value::as_f64) {
                r.check((m.font_size - s.max(1.0)).abs() <= TOLERANCE, || format!("{}: formula size", e.id));
            }
        }
        ElementKind::Image(i) => {
            let src = x.get("source").and_then(Value::as_array);
            let dims = src.and_then(|a| Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?)));
            r.check(dims == Some((u64::from(i.natural_width), u64::from(i.natural_height))), || {
                format!("{}: source size", e.id)
            });
            r.check(i.file_id.is_some(), || format!("{}: no file", e.id));
            r.check(i.crop.is_none() && !e.transform.flip_x && !e.transform.flip_y, || {
                format!("{}: crop or flip on a migrated image", e.id)
            });
        }
        ElementKind::Line(d) | ElementKind::Arrow(d) => {
            if let Some(n) = x.get("points").and_then(Value::as_u64) {
                r.check(d.points.len() as u64 == n, || format!("{}: {} points, old {n}", e.id, d.points.len()));
            }
            if let Some(h) = x.get("head_size").and_then(Value::as_f64) {
                r.check(d.head_size == Some(h), || format!("{}: head size {:?}, old {h}", e.id, d.head_size));
            }
            if let Some(c) = x.get("closed").and_then(Value::as_bool) {
                let want = c && d.points.len() >= 3;
                r.check(d.closed == want, || format!("{}: closed is {}, expected {want}", e.id, d.closed));
            }
        }
        ElementKind::Freehand(d) => {
            if let Some(n) = x.get("points").and_then(Value::as_u64) {
                r.check(d.points.len() as u64 == n, || format!("{}: {} points, old {n}", e.id, d.points.len()));
            }
            if let Some(n) = x.get("pressures").and_then(Value::as_u64) {
                r.check(d.pressures.len() as u64 == n, || {
                    format!("{}: {} pressures, old {n}", e.id, d.pressures.len())
                });
            }
            if let Some(c) = x.get("closed").and_then(Value::as_bool) {
                r.check(d.closed == c, || format!("{}: closed is {}, old {c}", e.id, d.closed));
            }
        }
        _ => {}
    }
}

/// Finds the shape object with this inner id anywhere in an old document (groups included).
fn old_shape_by_id<'a>(doc: &'a Value, id: &str) -> Option<&'a Value> {
    fn walk<'a>(v: &'a Value, id: &str, depth: usize) -> Option<&'a Value> {
        if depth > 80 {
            return None;
        }
        let inner = v.as_object()?.values().next()?;
        if inner.get("id").and_then(Value::as_str) == Some(id) {
            return Some(inner);
        }
        inner.get("children")?.as_array()?.iter().find_map(|c| walk(c, id, depth + 1))
    }
    doc.get("shapes")?.as_object()?.values().find_map(|s| walk(s, id, 0))
}

// ---------------------------------------------------------------------------------------------------------------
// CRDT snapshots
// ---------------------------------------------------------------------------------------------------------------

/// Checks an old Loro snapshot against its facts file and against the JSON document it was built from.
pub fn check_crdt_facts(
    name: &str,
    loro_bytes: &[u8],
    facts_json: &[u8],
    built_from: &[u8],
    measure: &dyn TextMeasure,
) -> FixtureReport {
    let mut r = FixtureReport::new(name);
    let Ok(facts) = serde_json::from_slice::<Value>(facts_json) else {
        r.fail("the facts file is not JSON");
        return r;
    };
    if let Some(sha) = facts.get("source_sha256").and_then(Value::as_str) {
        let actual = hex(&Sha256::digest(loro_bytes));
        r.check(actual == sha, || format!("the snapshot hashes to {actual}, the facts say {sha}"));
    }
    let snap = match legacy::migrate_snapshot(loro_bytes, &mut SeqIds::new(), measure) {
        Ok(i) => i,
        Err(e) => {
            r.fail(format!("snapshot migration failed: {e}"));
            return r;
        }
    };
    let Some(text) = std::str::from_utf8(built_from).ok() else {
        r.fail("the built-from document is not UTF-8");
        return r;
    };
    let json = match legacy::migrate_json(text, &mut SeqIds::new(), measure) {
        Ok(i) => i,
        Err(e) => {
            r.fail(format!("migration of the built-from document failed: {e}"));
            return r;
        }
    };
    if let Some(n) = facts.get("name").and_then(Value::as_str) {
        r.check(snap.document.name == n, || format!("name is {:?}, expected {n:?}", snap.document.name));
    }
    if let Some(n) = facts.get("shape_count").and_then(Value::as_u64) {
        let top = facts.get("z_order").and_then(Value::as_array).map_or(0, Vec::len) as u64;
        r.check(n == top, || format!("facts shape_count {n} != z_order length {top}"));
    }
    let (a, b) = (&snap.document, &json.document);
    r.check(a.elements.len() == b.elements.len(), || {
        format!("{} elements from the snapshot, {} from the JSON", a.elements.len(), b.elements.len())
    });
    let group_sets = |d: &Document| -> BTreeSet<Vec<String>> {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in &d.elements {
            for g in &e.group_ids {
                m.entry(g.to_string()).or_default().push(e.id.to_string());
            }
        }
        m.into_values().collect()
    };
    r.check(group_sets(a) == group_sets(b), || "group membership differs between snapshot and JSON".to_owned());
    r.check(a.files.keys().eq(b.files.keys()), || "image blobs differ between snapshot and JSON".to_owned());
    let rotated_group = json_has_rotated_group(text);
    for (x, y) in a.elements.iter().zip(&b.elements) {
        r.check(x.id == y.id && x.tag() == y.tag(), || format!("{} / {}: id or kind differs", x.id, y.id));
        let mut x2 = x.clone();
        let mut y2 = y.clone();
        // What the old CRDT lost: closed flags. Seeds are random when absent, so equalise those fields.
        for e in [&mut x2, &mut y2] {
            e.index = drafftink_scene::ZIndex::first();
            match &mut e.kind {
                ElementKind::Line(d) => d.closed = false,
                ElementKind::Freehand(d) => d.closed = false,
                _ => {}
            }
        }
        // `closed` also affects nothing else in the geometry; group rotation changes boxes, so skip those.
        if rotated_group && !x.group_ids.is_empty() {
            r.skip("elements in groups of a document with a rotated group (the old CRDT dropped group rotation)");
            continue;
        }
        r.check(x2 == y2, || format!("{}: differs between snapshot and JSON beyond the known CRDT losses", x.id));
    }
    r
}

fn json_has_rotated_group(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|v| {
        fn walk(v: &Value, depth: usize) -> bool {
            if depth > 80 {
                return false;
            }
            let Some(inner) = v.as_object().and_then(|o| o.get("Group")) else { return false };
            inner.get("rotation").and_then(Value::as_f64).is_some_and(|r| r.abs() > ROTATION_EPS)
                || inner.get("children").and_then(Value::as_array).is_some_and(|c| c.iter().any(|x| walk(x, depth + 1)))
        }
        v.get("shapes").and_then(Value::as_object).is_some_and(|s| s.values().any(|x| walk(x, 0)))
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Directory walk
// ---------------------------------------------------------------------------------------------------------------

/// Checks every legacy fixture under `dir` (the `tests/fixtures` directory) and the hashes in `MANIFEST.json`,
/// using [`EstimateMeasure`]. Not available on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn check_fixture_facts(dir: &std::path::Path) -> Report {
    check_fixture_facts_with(dir, &EstimateMeasure)
}

/// Like [`check_fixture_facts`] with a given text measure.
#[cfg(not(target_arch = "wasm32"))]
pub fn check_fixture_facts_with(dir: &std::path::Path, measure: &dyn TextMeasure) -> Report {
    use std::fs;
    let mut report = Report::default();
    check_manifest(dir, &mut report);
    let legacy_dir = dir.join("legacy");
    let Ok(entries) = fs::read_dir(&legacy_dir) else {
        report.manifest_failures.push(format!("cannot read {}", legacy_dir.display()));
        return report;
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.contains(".facts.json"))
        .collect();
    names.sort();
    let read = |p: &std::path::Path| -> Option<Vec<u8>> {
        let bytes = fs::read(p).ok()?;
        if p.extension().is_some_and(|e| e == "gz") { legacy::gunzip(&bytes).ok() } else { Some(bytes) }
    };
    for n in names {
        let stem = n.split(".facts.json").next().unwrap_or(&n).to_owned();
        let gz = n.ends_with(".gz");
        let doc_name = if gz { format!("{stem}.json.gz") } else { format!("{stem}.json") };
        match (read(&legacy_dir.join(&doc_name)), read(&legacy_dir.join(&n))) {
            (Some(doc), Some(facts)) => report.fixtures.push(check_legacy_facts(&stem, &doc, &facts, measure)),
            _ => report.manifest_failures.push(format!("cannot read {doc_name} or {n}")),
        }
    }
    if let Ok(entries) = fs::read_dir(legacy_dir.join("crdt")) {
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".loro"))
            .collect();
        names.sort();
        for n in names {
            let stem = n.trim_end_matches(".loro").to_owned();
            let loro = fs::read(legacy_dir.join("crdt").join(&n));
            let facts = fs::read(legacy_dir.join("crdt").join(format!("{stem}.facts.json")));
            let built = fs::read(legacy_dir.join(format!("{stem}.json")));
            match (loro, facts, built) {
                (Ok(l), Ok(f), Ok(b)) => {
                    report.fixtures.push(check_crdt_facts(&format!("crdt/{stem}"), &l, &f, &b, measure))
                }
                _ => report.manifest_failures.push(format!("cannot read the files of crdt/{stem}")),
            }
        }
    }
    report
}

#[cfg(not(target_arch = "wasm32"))]
fn check_manifest(dir: &std::path::Path, report: &mut Report) {
    let Ok(text) = std::fs::read_to_string(dir.join("MANIFEST.json")) else {
        report.manifest_failures.push("MANIFEST.json is missing".to_owned());
        return;
    };
    let Ok(m) = serde_json::from_str::<Value>(&text) else {
        report.manifest_failures.push("MANIFEST.json is not JSON".to_owned());
        return;
    };
    let mut check = |path: &str, sha: Option<&str>, size: Option<u64>| {
        let Ok(bytes) = std::fs::read(dir.join(path)) else {
            report.manifest_failures.push(format!("{path}: listed in MANIFEST.json but missing"));
            return;
        };
        if let Some(sha) = sha {
            let actual = hex(&Sha256::digest(&bytes));
            if actual != sha {
                report.manifest_failures.push(format!("{path}: sha256 is {actual}, MANIFEST.json says {sha}"));
            }
        }
        if let Some(size) = size
            && size != bytes.len() as u64
        {
            report.manifest_failures.push(format!("{path}: {} bytes, MANIFEST.json says {size}", bytes.len()));
        }
    };
    if let Some(g) = m.get("generated").and_then(Value::as_object) {
        for (path, v) in g {
            check(path, v.get("sha256").and_then(Value::as_str), v.get("bytes").and_then(Value::as_u64));
        }
    }
    if let Some(g) = m.get("inputs").and_then(Value::as_object) {
        for (path, v) in g {
            check(path, v.as_str(), None);
        }
    }
}
