//! Compares the calligraphy brush and the simulated pressure with the old app.
//!
//! `tests/data/brush_old.json` is written by the scratch tool `brush-compare`
//! (`/home/ubuntu/git/drafftink-spikes/brush-compare`, see its source header). The tool drives the
//! old `ToolManager` of `drafft-ink` 8ce40ab with scripted pointer events (calligraphy mode on, the
//! real code) and runs its `compute_pressure` with an injected clock. To regenerate: `cargo build
//! --release` in the scratch project, then `brush-compare crates/edit/tests/data/brush_old.json`.
//!
//! The old brush advanced its spring by 1/60 s per event, so every script here runs at 60 events a
//! second where the old and the new brush are the same function; the scripts with irregular times
//! only feed the pressure, which the old code computed from real time too.

// Test helpers (not `#[test]` fns) may panic on malformed fixtures.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use drafftink_edit::brush::{StrokeSample, StrokeSampler};
use drafftink_geom::kurbo::Point;
use drafftink_scene::{Brush, PressureMode};
use serde_json::Value;

/// The data is the old code's `f64`s written with their shortest round trip. What differs is the order
/// of a few float operations: `(t - t0) / 1000` against `t / 1000 - t0 / 1000`.
const TOL: f64 = 1e-9;

fn load() -> Value {
    serde_json::from_str(include_str!("data/brush_old.json")).expect("brush_old.json is valid JSON")
}

fn num(v: &Value) -> f64 {
    v.as_f64().expect("number")
}

fn replay(case: &Value) -> Vec<StrokeSample> {
    let brush = if case["calligraphy"].as_bool().expect("bool") { Brush::Calligraphy } else { Brush::Normal };
    let mode = if case["simulate"].as_bool().expect("bool") { PressureMode::Simulated } else { PressureMode::Uniform };
    let events = case["events"].as_array().expect("events");
    let at = |e: &Value| Point::new(num(&e[0]), num(&e[1]));
    let (first, rest) = events.split_first().expect("a first event");
    let (mut sampler, start) = StrokeSampler::start(brush, mode, at(first), num(&first[2]), None);
    let mut out = vec![start];
    out.extend(rest.iter().filter_map(|e| sampler.push(at(e), num(&e[2]), None)));
    out
}

fn compare(case: &Value) -> Result<(), String> {
    let name = case["name"].as_str().expect("name");
    let new = replay(case);
    let old_points = case["points"].as_array().expect("points");
    let old_pressures = case["pressures"].as_array().expect("pressures");
    let simulate = case["simulate"].as_bool().expect("bool");
    if new.len() != old_points.len() {
        return Err(format!("{name}: {} points, the old app kept {}", new.len(), old_points.len()));
    }
    let mut worst = 0.0_f64;
    for (i, (n, (op, pr))) in new.iter().zip(old_points.iter().zip(old_pressures)).enumerate() {
        let d = (n.point.x - num(&op[0])).abs().max((n.point.y - num(&op[1])).abs());
        worst = worst.max(d);
        if simulate {
            let got = n.pressure.ok_or_else(|| format!("{name}[{i}]: no pressure"))?;
            worst = worst.max((got - num(pr)).abs());
        } else if n.pressure.is_some() {
            return Err(format!("{name}[{i}]: a pressure without the mode"));
        }
    }
    if worst > TOL { Err(format!("{name}: worst difference {worst:e}")) } else { Ok(()) }
}

#[test]
fn the_calligraphy_brush_keeps_the_points_the_old_app_kept() {
    let doc = load();
    let cases: Vec<&Value> =
        doc["cases"].as_array().expect("cases").iter().filter(|c| c["calligraphy"] == true).collect();
    assert!(cases.len() >= 4, "the fixture holds the calligraphy cases");
    let failures: Vec<String> = cases.into_iter().filter_map(|c| compare(c).err()).collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn the_simulated_pressure_is_the_pressure_the_old_app_computed() {
    let doc = load();
    let cases: Vec<&Value> = doc["cases"].as_array().expect("cases").iter().filter(|c| c["simulate"] == true).collect();
    assert!(cases.len() >= 4, "the fixture holds the pressure cases");
    let failures: Vec<String> = cases.into_iter().filter_map(|c| compare(c).err()).collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn the_old_strokes_do_use_the_range_of_the_pressure() {
    // Guards the fixture: a speed model that never leaves 1 would pass the comparison above for any port.
    let doc = load();
    let case = doc["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|c| c["name"] == "simulated_loop_60hz")
        .expect("the loop case");
    let ps: Vec<f64> = case["pressures"].as_array().expect("pressures").iter().map(num).collect();
    let (lo, hi) = ps.iter().fold((1.0_f64, 0.0_f64), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    assert!(lo < 0.4 && hi > 0.99, "{lo} {hi}");
}

#[test]
fn the_calligraphy_stroke_lags_the_pointer_like_the_old_one() {
    let doc = load();
    let case = doc["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|c| c["name"] == "calligraphy_straight_60hz")
        .expect("the straight case");
    let last_event = case["events"].as_array().and_then(|e| e.last()).map(|e| num(&e[0])).expect("event");
    let last_point = case["points"].as_array().and_then(|e| e.last()).map(|e| num(&e[0])).expect("point");
    assert!(last_event - last_point > 50.0, "{last_event} {last_point}");
}
