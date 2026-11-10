//! Layered (Sugiyama style) layout: cycle removal, layering, crossing reduction, coordinate assignment.
//!
//! 1. A depth-first search finds back edges and reverses them, so the graph is acyclic.
//! 2. Longest-path layering, then sources are pulled down next to their first successor.
//! 3. Edges that span several layers get dummy nodes, one per layer crossed.
//! 4. Barycenter sweeps down and up reorder every layer; the order with the fewest crossings wins.
//! 5. Coordinates across the layers come from repeated pooling of adjacent violators: each layer moves every node
//!    toward the mean of its neighbours' positions while keeping the order and the spacing.
//!
//! Everything is deterministic: ties are broken by index and no hash map is iterated.

use std::collections::BTreeSet;

use kurbo::{Point, Size};

use super::{Direction, LayoutGraph, LayoutOpts, LayoutResult};

const MAX_SWEEPS: usize = 24;
const PLACEMENT_PASSES: usize = 30;
/// Cross-axis room reserved for an edge passing through a layer.
const DUMMY_CROSS: f64 = 10.0;

struct LNode {
    orig: Option<usize>,
    layer: usize,
    cross: f64,
    main: f64,
}

/// Reverses the back edges of a depth-first search. Returns the oriented edges (sorted, no duplicates, no self
/// loops) and the nodes in topological order.
fn break_cycles(n: usize, edges: &BTreeSet<(usize, usize)>) -> (Vec<(usize, usize)>, Vec<usize>) {
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in edges {
        out[a].push(b);
    }
    // 0 = new, 1 = on the stack, 2 = done.
    let mut state = vec![0u8; n];
    let mut finished: Vec<usize> = Vec::with_capacity(n);
    let mut kept: BTreeSet<(usize, usize)> = BTreeSet::new();
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        state[start] = 1;
        while let Some(&mut (u, ref mut next)) = stack.last_mut() {
            if let Some(&v) = out[u].get(*next) {
                *next += 1;
                match state[v] {
                    0 => {
                        kept.insert((u, v));
                        state[v] = 1;
                        stack.push((v, 0));
                    }
                    1 => {
                        kept.insert((v, u));
                    }
                    _ => {
                        kept.insert((u, v));
                    }
                }
            } else {
                state[u] = 2;
                finished.push(u);
                stack.pop();
            }
        }
    }
    finished.reverse();
    (kept.into_iter().collect(), finished)
}

fn count_crossings(layers: &[Vec<usize>], downs: &[Vec<usize>], pos: &[usize]) -> usize {
    let mut total = 0usize;
    for l in 0..layers.len().saturating_sub(1) {
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for &u in &layers[l] {
            for &v in &downs[u] {
                pairs.push((pos[u], pos[v]));
            }
        }
        pairs.sort_unstable();
        let width = layers[l + 1].len();
        let mut tree = vec![0usize; width + 1];
        for (seen, &(_, pv)) in pairs.iter().enumerate() {
            // Number of earlier edges whose lower end is right of this one.
            let mut le = 0usize;
            let mut i = pv + 1;
            while i > 0 {
                le += tree[i];
                i -= i & i.wrapping_neg();
            }
            total += seen - le;
            let mut i = pv + 1;
            while i <= width {
                tree[i] += 1;
                i += i & i.wrapping_neg();
            }
        }
    }
    total
}

fn set_pos(layers: &[Vec<usize>], pos: &mut [usize]) {
    for layer in layers {
        for (i, &v) in layer.iter().enumerate() {
            pos[v] = i;
        }
    }
}

/// Pool-adjacent-violators: the non-decreasing sequence closest (weighted least squares) to `target`.
fn isotonic(target: &[f64], weight: &[f64]) -> Vec<f64> {
    // Blocks of (weighted sum, total weight, count).
    let mut blocks: Vec<(f64, f64, usize)> = Vec::with_capacity(target.len());
    for (t, w) in target.iter().zip(weight) {
        blocks.push((t * w, *w, 1));
        while blocks.len() >= 2 {
            let (b, a) = (blocks[blocks.len() - 1], blocks[blocks.len() - 2]);
            if a.0 / a.1 <= b.0 / b.1 {
                break;
            }
            blocks.truncate(blocks.len() - 2);
            blocks.push((a.0 + b.0, a.1 + b.1, a.2 + b.2));
        }
    }
    let mut out = Vec::with_capacity(target.len());
    for (sum, w, count) in blocks {
        out.extend(std::iter::repeat_n(sum / w, count));
    }
    out
}

pub(super) fn layered(g: &LayoutGraph, o: &LayoutOpts) -> LayoutResult {
    let n = g.sizes.len();
    if n == 0 {
        return LayoutResult {
            positions: Vec::new(),
            waypoints: vec![Vec::new(); g.edges.len()],
            flow: Some(o.direction),
        };
    }
    let vertical = matches!(o.direction, Direction::Down | Direction::Up);
    let mirrored = matches!(o.direction, Direction::Up | Direction::Left);
    let main_of = |s: Size| if vertical { s.height } else { s.width };
    let cross_of = |s: Size| if vertical { s.width } else { s.height };

    // 1. Acyclic edges.
    let edge_set: BTreeSet<(usize, usize)> = g.edges.iter().copied().filter(|(a, b)| a != b).collect();
    let (dag, topo) = break_cycles(n, &edge_set);

    // 2. Layers.
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(u, v) in &dag {
        succ[u].push(v);
        pred[v].push(u);
    }
    let mut rank = vec![0usize; n];
    for &u in &topo {
        for &v in &succ[u] {
            rank[v] = rank[v].max(rank[u] + 1);
        }
    }
    for &u in topo.iter().rev() {
        if pred[u].is_empty() && !succ[u].is_empty() {
            let first = succ[u].iter().map(|&v| rank[v]).min().unwrap_or(1);
            rank[u] = first.saturating_sub(1);
        }
    }
    let nlayers = rank.iter().copied().max().unwrap_or(0) + 1;

    // 3. Dummy nodes.
    let mut nodes: Vec<LNode> = (0..n)
        .map(|i| LNode { orig: Some(i), layer: rank[i], cross: cross_of(g.sizes[i]), main: main_of(g.sizes[i]) })
        .collect();
    let mut ups: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut downs: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut chains: Vec<((usize, usize), Vec<usize>)> = Vec::new();
    for &(u, v) in &dag {
        let mut chain = vec![u];
        for l in rank[u] + 1..rank[v] {
            nodes.push(LNode { orig: None, layer: l, cross: DUMMY_CROSS, main: 0.0 });
            ups.push(Vec::new());
            downs.push(Vec::new());
            chain.push(nodes.len() - 1);
        }
        chain.push(v);
        for w in chain.windows(2) {
            downs[w[0]].push(w[1]);
            ups[w[1]].push(w[0]);
        }
        chains.push(((u, v), chain));
    }
    let total = nodes.len();

    // 4. Order within layers.
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); nlayers];
    for (i, nd) in nodes.iter().enumerate() {
        layers[nd.layer].push(i);
    }
    let mut pos = vec![0usize; total];
    set_pos(&layers, &mut pos);
    let mut best = layers.clone();
    let mut best_c = count_crossings(&layers, &downs, &pos);
    let mut stale = 0;
    for sweep in 0..MAX_SWEEPS {
        if best_c == 0 {
            break;
        }
        let down = sweep % 2 == 0;
        let order: Vec<usize> =
            if down { (1..nlayers).collect() } else { (0..nlayers.saturating_sub(1)).rev().collect() };
        for l in order {
            let mut keyed: Vec<(f64, usize, usize)> = layers[l]
                .iter()
                .map(|&v| {
                    let nb = if down { &ups[v] } else { &downs[v] };
                    let key = if nb.is_empty() {
                        pos[v] as f64
                    } else {
                        nb.iter().map(|&u| pos[u] as f64).sum::<f64>() / nb.len() as f64
                    };
                    (key, pos[v], v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            layers[l] = keyed.into_iter().map(|k| k.2).collect();
            for (i, &v) in layers[l].iter().enumerate() {
                pos[v] = i;
            }
        }
        let c = count_crossings(&layers, &downs, &pos);
        if c < best_c {
            best_c = c;
            best = layers.clone();
            stale = 0;
        } else {
            stale += 1;
            if stale >= 4 {
                break;
            }
        }
    }
    let layers = best;

    // 5. Coordinates across the layers.
    let gap = o.node_gap;
    // Each node keeps half the gap around it; an edge passing through takes only a sixth, so it still leaves
    // at least `gap` between the two real nodes next to it.
    let half_gap = |v: usize| if nodes[v].orig.is_some() { gap / 2.0 } else { gap / 6.0 };
    let sep = |a: usize, b: usize| (nodes[a].cross + nodes[b].cross) / 2.0 + half_gap(a) + half_gap(b);
    let mut x = vec![0.0f64; total];
    for layer in &layers {
        let mut cursor = 0.0;
        for (i, &v) in layer.iter().enumerate() {
            if i > 0 {
                cursor += sep(layer[i - 1], v);
            }
            x[v] = cursor;
        }
        let mid = layer.last().map_or(0.0, |&v| x[v] / 2.0);
        for &v in layer {
            x[v] -= mid;
        }
    }
    for pass in 0..PLACEMENT_PASSES {
        let order: Vec<usize> = if pass % 2 == 0 { (0..nlayers).collect() } else { (0..nlayers).rev().collect() };
        for l in order {
            let layer = &layers[l];
            if layer.is_empty() {
                continue;
            }
            let mut offsets = Vec::with_capacity(layer.len());
            let mut acc = 0.0;
            for (i, &v) in layer.iter().enumerate() {
                if i > 0 {
                    acc += sep(layer[i - 1], v);
                }
                offsets.push(acc);
                let _ = v;
            }
            let mut target = Vec::with_capacity(layer.len());
            let mut weight = Vec::with_capacity(layer.len());
            for (i, &v) in layer.iter().enumerate() {
                let nb: Vec<usize> = ups[v].iter().chain(&downs[v]).copied().collect();
                let (desired, w) = if nb.is_empty() {
                    (x[v], 0.01)
                } else {
                    // Edges between two dummy nodes pull harder, so long edges run straight; the end of a long
                    // edge pulls its real node less than a short edge does, so chains of real nodes stay in line.
                    let mut sum = 0.0;
                    let mut wsum = 0.0;
                    for &u in &nb {
                        let w = match (nodes[v].orig.is_none(), nodes[u].orig.is_none()) {
                            (true, true) => 4.0,
                            (false, false) => 1.0,
                            _ => 0.25,
                        };
                        sum += w * x[u];
                        wsum += w;
                    }
                    (sum / wsum, wsum)
                };
                target.push(desired - offsets[i]);
                weight.push(w);
            }
            let y = isotonic(&target, &weight);
            for (i, &v) in layer.iter().enumerate() {
                x[v] = y[i] + offsets[i];
            }
        }
    }

    // Layer thickness and main-axis positions.
    let mut thickness = vec![0.0f64; nlayers];
    for nd in &nodes {
        thickness[nd.layer] = thickness[nd.layer].max(nd.main);
    }
    let mut layer_start = vec![0.0f64; nlayers];
    let mut acc = 0.0;
    for l in 0..nlayers {
        layer_start[l] = acc;
        acc += thickness[l] + o.layer_gap;
    }
    let total_main = (acc - o.layer_gap).max(0.0);

    // Back to x and y.
    let min_cross = (0..total).map(|v| x[v] - nodes[v].cross / 2.0).fold(f64::INFINITY, f64::min);
    let to_xy = |cross_center: f64, main_center: f64| -> Point {
        let cross = cross_center - min_cross;
        let main = if mirrored { total_main - main_center } else { main_center };
        if vertical { Point::new(cross, main) } else { Point::new(main, cross) }
    };
    let positions: Vec<Point> = (0..n)
        .map(|i| {
            let nd = &nodes[i];
            let centre_main = layer_start[nd.layer] + thickness[nd.layer] / 2.0;
            let c = to_xy(x[i], centre_main);
            Point::new(c.x - g.sizes[i].width / 2.0, c.y - g.sizes[i].height / 2.0)
        })
        .collect();

    // Waypoints: the dummy nodes of each edge, in the direction the edge was given.
    let waypoints = g
        .edges
        .iter()
        .map(|&(a, b)| {
            let find = |key: (usize, usize)| chains.binary_search_by(|(k, _)| k.cmp(&key)).ok();
            let pts = |chain: &[usize]| -> Vec<Point> {
                chain
                    .iter()
                    .filter(|&&v| nodes[v].orig.is_none())
                    .map(|&v| to_xy(x[v], layer_start[nodes[v].layer] + thickness[nodes[v].layer] / 2.0))
                    .collect()
            };
            if let Some(i) = find((a, b)) {
                pts(&chains[i].1)
            } else if let Some(i) = find((b, a)) {
                let mut p = pts(&chains[i].1);
                p.reverse();
                p
            } else {
                Vec::new()
            }
        })
        .collect();
    LayoutResult { positions, waypoints, flow: Some(o.direction) }
}
