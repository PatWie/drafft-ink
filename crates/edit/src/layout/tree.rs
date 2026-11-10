//! Tree layout (a forest of rooted trees), grid layout, and the shared main-axis placement.

use kurbo::{Point, Size};

use super::{Direction, LayoutError, LayoutGraph, LayoutOpts, LayoutResult};

/// The tree layout. Every node may have one incoming edge at most and there must be no cycle.
pub(super) fn tree(g: &LayoutGraph, o: &LayoutOpts) -> Result<LayoutResult, LayoutError> {
    let n = g.sizes.len();
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in &g.edges {
        if a == b || parent[b].is_some() {
            return Err(LayoutError::NotForest { node: b });
        }
        parent[b] = Some(a);
        children[a].push(b);
    }
    let vertical = matches!(o.direction, Direction::Down | Direction::Up);
    let mirrored = matches!(o.direction, Direction::Up | Direction::Left);
    let main_of = |s: Size| if vertical { s.height } else { s.width };
    let cross_of = |s: Size| if vertical { s.width } else { s.height };

    // Pre-order from the roots (nodes without a parent), children in edge order.
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut depth = vec![0usize; n];
    let mut stack: Vec<usize> = (0..n).rev().filter(|&i| parent[i].is_none()).collect();
    while let Some(u) = stack.pop() {
        order.push(u);
        for &c in children[u].iter().rev() {
            depth[c] = depth[u] + 1;
            stack.push(c);
        }
    }
    if order.len() != n {
        // Some node is never reached: it sits on a cycle.
        let mut seen = vec![false; n];
        for &u in &order {
            seen[u] = true;
        }
        let node = seen.iter().position(|s| !s).unwrap_or(0);
        return Err(LayoutError::NotForest { node });
    }

    // Slot widths across the layout, children before parents.
    let gap = o.node_gap;
    let mut extent = vec![0.0f64; n];
    for &u in order.iter().rev() {
        let kids = &children[u];
        let block: f64 = kids.iter().map(|&c| extent[c]).sum::<f64>() + gap * kids.len().saturating_sub(1) as f64;
        extent[u] = block.max(cross_of(g.sizes[u]));
    }
    // Slot starts, roots left to right.
    let mut start = vec![0.0f64; n];
    let mut cursor = 0.0;
    for &u in &order {
        if parent[u].is_none() {
            start[u] = cursor;
            cursor += extent[u] + gap;
        }
    }
    for &u in &order {
        let kids = &children[u];
        let block: f64 = kids.iter().map(|&c| extent[c]).sum::<f64>() + gap * kids.len().saturating_sub(1) as f64;
        let mut at = start[u] + (extent[u] - block) / 2.0;
        for &c in kids {
            start[c] = at;
            at += extent[c] + gap;
        }
    }
    let centre: Vec<f64> = (0..n).map(|u| start[u] + extent[u] / 2.0).collect();

    let (layer_start, thickness, total_main) = main_axis(g, &depth, o.layer_gap, main_of);
    let positions = (0..n)
        .map(|i| {
            let s = g.sizes[i];
            let main_centre = layer_start[depth[i]] + thickness[depth[i]] / 2.0;
            let main_centre = if mirrored { total_main - main_centre } else { main_centre };
            let (cx, cy) = if vertical { (centre[i], main_centre) } else { (main_centre, centre[i]) };
            Point::new(cx - s.width / 2.0, cy - s.height / 2.0)
        })
        .collect();
    Ok(LayoutResult { positions, waypoints: vec![Vec::new(); g.edges.len()], flow: Some(o.direction) })
}

/// Start and thickness of every layer (nodes with the same `layer[i]`) and the total length along the main axis.
fn main_axis(
    g: &LayoutGraph,
    layer: &[usize],
    layer_gap: f64,
    main_of: impl Fn(Size) -> f64,
) -> (Vec<f64>, Vec<f64>, f64) {
    let nlayers = layer.iter().copied().max().map_or(0, |m| m + 1);
    let mut thickness = vec![0.0f64; nlayers];
    for (i, &l) in layer.iter().enumerate() {
        thickness[l] = thickness[l].max(main_of(g.sizes[i]));
    }
    let mut start = vec![0.0f64; nlayers];
    let mut acc = 0.0;
    for l in 0..nlayers {
        start[l] = acc;
        acc += thickness[l] + layer_gap;
    }
    (start, thickness, (acc - layer_gap).max(0.0))
}

/// Grid layout: row by row in node order. The number of columns is `opts.columns` or about the square root of the
/// node count. Each column is as wide as its widest node and each row as tall as its tallest; nodes are centred in
/// their cell.
pub(super) fn grid(g: &LayoutGraph, o: &LayoutOpts) -> LayoutResult {
    let n = g.sizes.len();
    let columns = match o.columns {
        Some(c) if c >= 1 => (c as usize).min(n.max(1)),
        _ => ((n as f64).sqrt().ceil() as usize).max(1),
    };
    let rows = n.div_ceil(columns);
    let mut col_w = vec![0.0f64; columns];
    let mut row_h = vec![0.0f64; rows];
    for (i, s) in g.sizes.iter().enumerate() {
        col_w[i % columns] = col_w[i % columns].max(s.width);
        row_h[i / columns] = row_h[i / columns].max(s.height);
    }
    let mut col_x = vec![0.0f64; columns];
    for c in 1..columns {
        col_x[c] = col_x[c - 1] + col_w[c - 1] + o.node_gap;
    }
    let mut row_y = vec![0.0f64; rows];
    for r in 1..rows {
        row_y[r] = row_y[r - 1] + row_h[r - 1] + o.layer_gap;
    }
    let positions = g
        .sizes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (c, r) = (i % columns, i / columns);
            Point::new(col_x[c] + (col_w[c] - s.width) / 2.0, row_y[r] + (row_h[r] - s.height) / 2.0)
        })
        .collect();
    LayoutResult { positions, waypoints: vec![Vec::new(); g.edges.len()], flow: None }
}
