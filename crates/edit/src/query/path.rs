//! Elements hit by an eraser path.

use std::collections::BTreeSet;

use drafftink_geom::kurbo::Point;
use drafftink_scene::{ElementId, SceneRead};

use super::{AtOpts, elements_at};
use crate::group::units::{Reverse, ensure_unlocked, sort_z, with_companions};

/// The smallest number of samples per radius along an eraser stroke. Two samples per radius keep a stroke from
/// skipping a thin element.
const SAMPLES_PER_RADIUS: f64 = 2.0;
/// No path is sampled with more points than this.
const MAX_SAMPLES: usize = 20_000;

/// The unlocked elements hit by a stroke of `radius` along `path` (world units), by exact hit testing. Group
/// members are hit as themselves; a bound text counts as its container. A hit that has a locked member in its
/// closure (its bound text, or the children of a frame) is left out. The result is in z-order.
pub fn hit_by_path<R: SceneRead>(scene: &R, path: &[Point], radius: f64) -> Vec<ElementId> {
    let radius = if radius.is_finite() { radius.max(0.0) } else { 0.0 };
    let opts = AtOpts { tolerance: radius, all: true, deep: true, include_locked: false };
    let mut hit: BTreeSet<ElementId> = BTreeSet::new();
    let step = (radius / SAMPLES_PER_RADIUS).max(0.5);
    let mut samples = 0usize;
    let probe = |p: Point, hit: &mut BTreeSet<ElementId>| {
        for h in elements_at(scene, p, &opts) {
            hit.insert(h.id);
        }
    };
    for (i, p) in path.iter().enumerate() {
        probe(*p, &mut hit);
        let Some(next) = path.get(i + 1) else { continue };
        let len = p.distance(*next);
        if !len.is_finite() {
            continue;
        }
        let n = (len / step).ceil();
        let n = if n.is_finite() { n.min(MAX_SAMPLES as f64) as usize } else { 0 };
        for k in 1..n {
            samples += 1;
            if samples > MAX_SAMPLES {
                break;
            }
            let t = k as f64 / n as f64;
            probe(p.lerp(*next, t), &mut hit);
        }
    }
    // A hit whose whole closure (bound text, frame children) is not unlocked is skipped, never half-deleted.
    let rel = Reverse::of(scene);
    let mut out: Vec<ElementId> = hit
        .into_iter()
        .filter(|id| ensure_unlocked(scene, &with_companions(scene, &rel, std::slice::from_ref(id), true)).is_ok())
        .collect();
    sort_z(scene, &mut out);
    out
}
