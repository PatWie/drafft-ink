//! Compares the new crate with the old app's sketch code.
//!
//! `tests/data/old_app.json` is written by the scratch tool `sketch-compare`
//! (`/home/ubuntu/git/drafftink-spikes/sketch-compare`, see its source header). The tool runs the
//! old functions `apply_hand_drawn_effect`, `generate_fill_pattern` and the body of
//! `render_freehand_with_pressure` from `drafft-ink` 8ce40ab (roughr 0.12, kurbo 0.12) at zoom 1,
//! on the old `Shape::to_path` outlines, and dumps the resulting paths with 6 decimals.
//!
//! To regenerate: `cargo build --release` in the scratch project, then
//! `sketch-compare crates/sketch/tests/data/old_app.json`.
//!
//! Tolerance: coordinates must agree within `TOL` (the data is rounded to 1e-6). The outline noise
//! passes, rounded corners, arrows, spline and pressure outlines are exact up to that rounding.
//! Fill patterns come from roughr, which changed between 0.12 (old) and 0.14 (new); any difference
//! is reported by `fill_patterns_match_old_app`.

use drafftink_sketch::{
    Arrowhead, Curve, FillKind, FillStyle, PressureProfile, Roundness, SketchGeometry, SketchInput, SketchParams,
    SketchShape, sketch,
};
use kurbo::{BezPath, PathEl, Point};
use serde_json::Value;

const TOL: f64 = 2e-6;

fn load() -> Value {
    let text = include_str!("data/old_app.json");
    serde_json::from_str(text).expect("old_app.json is valid JSON")
}

fn f(v: &Value) -> f64 {
    v.as_f64().expect("number")
}

fn points(v: &Value) -> Vec<Point> {
    v.as_array().expect("array").iter().map(|p| Point::new(f(&p[0]), f(&p[1]))).collect()
}

fn fill_style(name: Option<&str>) -> FillStyle {
    match name {
        None => FillStyle::None,
        Some("solid") => FillStyle::Solid,
        Some("hachure") => FillStyle::Hachure,
        Some("crosshatch") => FillStyle::CrossHatch,
        Some("zigzag") => FillStyle::ZigZag,
        Some("dots") => FillStyle::Dots,
        Some("dashed") => FillStyle::Dashed,
        Some("zigzagline") => FillStyle::ZigZagLine,
        Some(other) => panic!("unknown fill {other}"),
    }
}

fn input_of(case: &Value) -> SketchInput {
    let s = &case["shape"];
    let curve = |smooth: bool| if smooth { Curve::Smooth } else { Curve::Straight };
    let shape = match s["kind"].as_str().expect("kind") {
        "rect" => {
            let r = f(&s["radius"]);
            SketchShape::Rect {
                width: f(&s["width"]),
                height: f(&s["height"]),
                roundness: if r > 0.0 { Roundness::Radius(r) } else { Roundness::Sharp },
            }
        }
        "ellipse" => SketchShape::Ellipse { width: f(&s["width"]), height: f(&s["height"]) },
        "line" => SketchShape::Polyline {
            points: points(&s["points"]),
            curve: curve(s["smooth"].as_bool().expect("bool")),
            closed: s["closed"].as_bool().expect("bool"),
        },
        "arrow" => SketchShape::Arrow {
            points: points(&s["points"]),
            curve: curve(s["smooth"].as_bool().expect("bool")),
            start: Arrowhead::NONE,
            end: Arrowhead::ARROW,
            head_size: f(&s["head_size"]),
        },
        "freehand" => SketchShape::Freehand {
            points: points(&s["points"]),
            pressures: vec![],
            closed: s["closed"].as_bool().expect("bool"),
            profile: PressureProfile::default(),
        },
        other => panic!("unknown kind {other}"),
    };
    SketchInput::new(
        shape,
        SketchParams {
            roughness: f(&case["roughness"]),
            seed: u32::try_from(case["seed"].as_u64().expect("seed")).expect("u32"),
            stroke_width: f(&case["stroke_width"]),
            fill: fill_style(case["fill"].as_str()),
            ..SketchParams::default()
        },
    )
}

/// Largest coordinate difference between a dumped path and a path, or `Err` for a structural
/// difference (different element kinds or counts).
fn diff(old: &Value, new: &BezPath) -> Result<f64, String> {
    let old = old.as_array().expect("path array");
    let new = new.elements();
    if old.len() != new.len() {
        return Err(format!("element count: old {} new {}", old.len(), new.len()));
    }
    let mut worst = 0.0f64;
    for (i, (o, n)) in old.iter().zip(new).enumerate() {
        let tag = o[0].as_str().expect("tag");
        let (new_tag, coords): (&str, Vec<f64>) = match n {
            PathEl::MoveTo(a) => ("M", vec![a.x, a.y]),
            PathEl::LineTo(a) => ("L", vec![a.x, a.y]),
            PathEl::QuadTo(a, b) => ("Q", vec![a.x, a.y, b.x, b.y]),
            PathEl::CurveTo(a, b, c) => ("C", vec![a.x, a.y, b.x, b.y, c.x, c.y]),
            PathEl::ClosePath => ("Z", vec![]),
        };
        if tag != new_tag {
            return Err(format!("element {i}: old {tag} new {new_tag}"));
        }
        for (k, c) in coords.iter().enumerate() {
            worst = worst.max((f(&o[k + 1]) - c).abs());
        }
    }
    Ok(worst)
}

fn check(name: &str, what: &str, old: &Value, new: &BezPath, failures: &mut Vec<String>, worst: &mut f64) {
    match diff(old, new) {
        Ok(d) => {
            *worst = worst.max(d);
            if d > TOL {
                failures.push(format!("{name} {what}: max diff {d}"));
            }
        }
        Err(e) => failures.push(format!("{name} {what}: {e}")),
    }
}

fn stroke_path(g: &SketchGeometry, i: usize) -> BezPath {
    g.strokes.get(i).map(|s| s.0.clone()).unwrap_or_default()
}

/// Outlines, fill regions: everything except roughr patterns.
#[test]
fn outlines_match_old_app() {
    let doc = load();
    let mut failures = Vec::new();
    let mut worst = 0.0f64;
    let mut checked = 0;
    for case in doc["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().expect("name");
        let g = sketch(&input_of(case));
        if f(&case["roughness"]) == 0.0 {
            if g.strokes.len() != 1 {
                failures.push(format!("{name}: expected one clean stroke, got {}", g.strokes.len()));
                continue;
            }
            check(name, "clean", &case["clean"], &stroke_path(&g, 0), &mut failures, &mut worst);
        } else {
            if g.strokes.len() != 2 {
                failures.push(format!("{name}: expected two strokes, got {}", g.strokes.len()));
                continue;
            }
            check(name, "pass0", &case["pass0"], &stroke_path(&g, 0), &mut failures, &mut worst);
            check(name, "pass1", &case["pass1"], &stroke_path(&g, 1), &mut failures, &mut worst);
        }
        checked += 1;
        if case.get("fill_region").is_some() {
            match g.fills.first() {
                Some((region, _)) => {
                    check(name, "fill_region", &case["fill_region"], region, &mut failures, &mut worst)
                }
                None => failures.push(format!("{name}: no fill emitted")),
            }
        }
    }
    println!("{checked} cases, worst coordinate difference {worst:e}");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert!(checked >= 100, "only {checked} cases");
}

/// Fill instructions have the old structure: solid fills draw the (0.3x rough) outline; patterns
/// draw it at 15 % alpha and then hatch lines of width `0.5 * stroke_width` clipped to it.
#[test]
fn fill_structure_matches_old_app() {
    let doc = load();
    let mut n = 0;
    for case in doc["cases"].as_array().expect("cases") {
        let Some(fill) = case["fill"].as_str() else { continue };
        let name = case["name"].as_str().expect("name");
        let g = sketch(&input_of(case));
        let sw = f(&case["stroke_width"]);
        if fill == "solid" {
            assert_eq!(g.fills.len(), 1, "{name}");
            assert_eq!(g.fills[0].1, FillKind::Solid, "{name}");
        } else {
            assert_eq!(g.fills.len(), 2, "{name}");
            assert_eq!(g.fills[0].1, FillKind::Tint { alpha: 0.15 }, "{name}");
            match &g.fills[1].1 {
                FillKind::Hatch { line_width, clip } => {
                    assert!((line_width - sw * 0.5).abs() < 1e-12, "{name}");
                    assert_eq!(clip, &g.fills[0].0, "{name}");
                }
                other => panic!("{name}: {other:?}"),
            }
        }
        n += 1;
    }
    assert!(n >= 40, "{n}");
}

/// roughr patterns against roughr 0.12 as used by the old app.
#[test]
fn fill_patterns_match_old_app() {
    let doc = load();
    let mut failures = Vec::new();
    let mut worst = 0.0f64;
    let mut n = 0;
    for case in doc["cases"].as_array().expect("cases") {
        let Some(old) = case.get("pattern") else { continue };
        let name = case["name"].as_str().expect("name");
        let g = sketch(&input_of(case));
        match g.fills.get(1) {
            Some((lines, _)) => check(name, "pattern", old, lines, &mut failures, &mut worst),
            None => failures.push(format!("{name}: no pattern emitted")),
        }
        n += 1;
    }
    println!("{n} patterns, worst coordinate difference {worst:e}");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert!(n >= 30, "{n}");
}

#[test]
fn pressure_outlines_match_old_app() {
    let doc = load();
    let mut failures = Vec::new();
    let mut worst = 0.0f64;
    for case in doc["pressure"].as_array().expect("pressure") {
        let name = case["name"].as_str().expect("name");
        let input = SketchInput::new(
            SketchShape::Freehand {
                points: points(&case["points"]),
                pressures: case["pressures"].as_array().expect("pressures").iter().map(f).collect(),
                closed: false,
                profile: PressureProfile::default(),
            },
            SketchParams { stroke_width: f(&case["stroke_width"]), ..SketchParams::default() },
        );
        let g = sketch(&input);
        assert!(g.strokes.is_empty(), "{name}: pressure strokes are fills");
        assert_eq!(g.fills.len(), 1, "{name}");
        assert_eq!(g.fills[0].1, FillKind::Ink, "{name}");
        check(name, "outline", &case["outline"], &g.fills[0].0, &mut failures, &mut worst);
    }
    println!("worst coordinate difference {worst:e}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
