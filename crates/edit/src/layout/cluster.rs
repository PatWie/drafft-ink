//! Layout with clusters: titled boxes (Mermaid subgraphs) that hold nodes and other clusters.
//!
//! The layout is hierarchical, from the innermost cluster out. Each cluster is laid out as a graph of its own
//! direct members; the result, with padding and a title band, is the size of the cluster as seen by the scope
//! around it. An edge is laid out in the innermost scope that holds both of its ends (each end lifted to the
//! member of that scope that contains it). So the members of a cluster always sit inside its box, and two
//! sibling boxes never overlap, because the layout of a scope does not let its members overlap.

use drafftink_geom::kurbo::{Point, Rect, Size, Vec2};

use super::{Direction, LayoutError, LayoutGraph, LayoutOpts, compute_layout};

/// Space between the content of a cluster and its box.
pub(super) const CLUSTER_PADDING: f64 = 22.0;
/// Height of the title band at the top of a cluster box.
pub(super) const CLUSTER_TITLE_BAND: f64 = 32.0;
/// A box is at least this wide, so that an empty cluster is still a box.
const CLUSTER_MIN_WIDTH: f64 = 120.0;
/// Deepest nesting of clusters.
pub(super) const MAX_CLUSTER_DEPTH: usize = 32;

/// One cluster.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ClusterIn {
    /// The cluster around this one.
    pub parent: Option<usize>,
    /// The size of the title text; the box is at least as wide.
    pub title: Size,
    /// The flow inside this cluster. Absent: the one of the scope around it.
    pub direction: Option<Direction>,
}

/// What to lay out. Items are numbered nodes first, then clusters: item `node_sizes.len() + c` is cluster `c`.
#[derive(Debug, Clone, Copy)]
pub(super) struct ClusteredInput<'a> {
    /// Size of every node.
    pub node_sizes: &'a [Size],
    /// The innermost cluster of every node.
    pub node_cluster: &'a [Option<usize>],
    /// The clusters.
    pub clusters: &'a [ClusterIn],
    /// Edges between items.
    pub edges: &'a [(usize, usize)],
}

/// Where everything goes. Positions are top-left corners, relative to the top-left of the whole.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ClusteredLayout {
    /// Every node.
    pub nodes: Vec<Point>,
    /// The box of every cluster.
    pub boxes: Vec<Rect>,
    /// Waypoints of every edge, as in [`super::LayoutResult::waypoints`].
    pub waypoints: Vec<Vec<Point>>,
    /// The flow direction of the outermost scope.
    pub flow: Option<Direction>,
}

/// The scope around an item: a cluster, or `None` for the top level.
fn scope_of(inp: &ClusteredInput<'_>, item: usize) -> Option<usize> {
    let n = inp.node_sizes.len();
    match item.checked_sub(n) {
        None => inp.node_cluster.get(item).copied().flatten(),
        Some(c) => inp.clusters.get(c).and_then(|c| c.parent),
    }
}

/// The scopes from the one around `item` to the top level (last), each with the item of the chain that sits in it.
fn chain(inp: &ClusteredInput<'_>, item: usize) -> Vec<(Option<usize>, usize)> {
    let n = inp.node_sizes.len();
    let mut out = Vec::new();
    let mut cur = item;
    loop {
        let scope = scope_of(inp, cur);
        out.push((scope, cur));
        match scope {
            Some(c) if out.len() <= MAX_CLUSTER_DEPTH + 1 => cur = n + c,
            _ => return out,
        }
    }
}

/// Where an edge is laid out: the innermost scope that holds both ends, and the two members of it.
struct EdgeHome {
    scope: Option<usize>,
    a: usize,
    b: usize,
}

fn home_of(inp: &ClusteredInput<'_>, a: usize, b: usize) -> EdgeHome {
    let ca = chain(inp, a);
    let cb = chain(inp, b);
    for &(scope, item_a) in &ca {
        if let Some(&(_, item_b)) = cb.iter().find(|(s, _)| *s == scope) {
            return EdgeHome { scope, a: item_a, b: item_b };
        }
    }
    EdgeHome { scope: None, a, b }
}

/// How deep each cluster is (0 = in the top level), or the first cluster that is part of a loop or too deep.
fn depths(clusters: &[ClusterIn]) -> Result<Vec<usize>, LayoutError> {
    let mut out = Vec::with_capacity(clusters.len());
    for (c, _) in clusters.iter().enumerate() {
        let mut depth = 0;
        let mut cur = clusters[c].parent;
        while let Some(p) = cur {
            depth += 1;
            let Some(next) = clusters.get(p) else { return Err(LayoutError::BadCluster { cluster: c }) };
            if depth > MAX_CLUSTER_DEPTH {
                return Err(LayoutError::BadCluster { cluster: c });
            }
            cur = next.parent;
        }
        out.push(depth);
    }
    Ok(out)
}

/// A node of the item, for error messages: the item itself, or the first node inside the cluster.
fn representative_node(inp: &ClusteredInput<'_>, item: usize) -> usize {
    let n = inp.node_sizes.len();
    let Some(c) = item.checked_sub(n) else { return item };
    (0..n).find(|&i| chain(inp, i).iter().any(|&(s, _)| s == Some(c))).unwrap_or(0)
}

/// Lays out `inp`. See the module docs.
pub(super) fn layout_clustered(inp: &ClusteredInput<'_>, opts: &LayoutOpts) -> Result<ClusteredLayout, LayoutError> {
    let n = inp.node_sizes.len();
    let k = inp.clusters.len();
    let depth = depths(inp.clusters)?;
    if inp.node_cluster.len() != n || inp.node_cluster.iter().flatten().any(|&c| c >= k) {
        return Err(LayoutError::BadCluster { cluster: k });
    }
    if let Some(edge) = inp.edges.iter().position(|&(a, b)| a >= n + k || b >= n + k) {
        return Err(LayoutError::BadEdge { edge });
    }
    // Members of every scope; the top level is the last entry.
    let slot = |scope: Option<usize>| scope.unwrap_or(k);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); k + 1];
    for item in 0..n + k {
        members[slot(scope_of(inp, item))].push(item);
    }
    let homes: Vec<EdgeHome> = inp.edges.iter().map(|&(a, b)| home_of(inp, a, b)).collect();

    // The flow of each scope, from the outside in.
    let mut flow = vec![opts.direction; k + 1];
    let mut by_depth: Vec<usize> = (0..k).collect();
    by_depth.sort_by_key(|&c| depth[c]);
    for &c in &by_depth {
        let outer = flow[slot(inp.clusters[c].parent)];
        flow[c] = inp.clusters[c].direction.unwrap_or(outer);
    }

    let mut size: Vec<Size> = inp.node_sizes.to_vec();
    size.resize(n + k, Size::ZERO);
    let mut rel = vec![Point::ZERO; n + k];
    let mut edge_points: Vec<(Option<usize>, Vec<Point>)> = vec![(None, Vec::new()); inp.edges.len()];
    let mut top_flow = None;

    // Innermost scopes first, the top level last.
    let order = by_depth.iter().rev().map(|&c| Some(c)).chain(std::iter::once(None));
    for scope in order {
        let items = &members[slot(scope)];
        if items.is_empty() {
            if let Some(c) = scope {
                size[n + c] = Size::new(
                    (inp.clusters[c].title.width + 2.0 * CLUSTER_PADDING).max(CLUSTER_MIN_WIDTH),
                    CLUSTER_TITLE_BAND + 2.0 * CLUSTER_PADDING,
                );
            }
            continue;
        }
        let local = |item: usize| items.iter().position(|&i| i == item);
        let edge_ids: Vec<usize> = (0..homes.len()).filter(|&e| homes[e].scope == scope).collect();
        let graph = LayoutGraph {
            sizes: items.iter().map(|&i| size[i]).collect(),
            edges: edge_ids.iter().filter_map(|&e| Some((local(homes[e].a)?, local(homes[e].b)?))).collect(),
            fixed: Vec::new(),
        };
        let scope_opts = LayoutOpts { direction: flow[slot(scope)], ..opts.clone() };
        let laid = compute_layout(&graph, &scope_opts).map_err(|e| match e {
            LayoutError::NotForest { node } => {
                LayoutError::NotForest { node: representative_node(inp, items.get(node).copied().unwrap_or(0)) }
            }
            LayoutError::BadSize { node } => {
                LayoutError::BadSize { node: representative_node(inp, items.get(node).copied().unwrap_or(0)) }
            }
            other => other,
        })?;
        let content = items
            .iter()
            .zip(&laid.positions)
            .map(|(&i, p)| Rect::from_origin_size(*p, size[i]))
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect::ZERO);
        let offset = match scope {
            None => Vec2::ZERO,
            Some(c) => {
                let inner_w = content.width().max(inp.clusters[c].title.width);
                let outer_w = (inner_w + 2.0 * CLUSTER_PADDING).max(CLUSTER_MIN_WIDTH);
                size[n + c] = Size::new(outer_w, content.height() + 2.0 * CLUSTER_PADDING + CLUSTER_TITLE_BAND);
                // The content is centred when the title is wider than it.
                Vec2::new(
                    CLUSTER_PADDING + (outer_w - 2.0 * CLUSTER_PADDING - content.width()) / 2.0,
                    CLUSTER_PADDING + CLUSTER_TITLE_BAND,
                )
            }
        };
        for (&i, p) in items.iter().zip(&laid.positions) {
            rel[i] = *p + offset;
        }
        let mut next_waypoints = laid.waypoints.into_iter();
        for &e in &edge_ids {
            // Edges whose ends are not both items of the scope (cannot happen by construction) have no waypoints.
            if local(homes[e].a).is_some() && local(homes[e].b).is_some() {
                let pts = next_waypoints.next().unwrap_or_default();
                edge_points[e] = (scope, pts.into_iter().map(|p| p + offset).collect());
            }
        }
        if scope.is_none() {
            top_flow = laid.flow;
        }
    }

    // From the outside in: absolute positions.
    let mut abs = rel.clone();
    for &c in &by_depth {
        let base = abs[n + c].to_vec2();
        for &i in &members[c] {
            abs[i] = rel[i] + base;
        }
    }
    let waypoints = edge_points
        .into_iter()
        .map(|(scope, pts)| {
            let base = scope.map_or(Vec2::ZERO, |c| abs[n + c].to_vec2());
            pts.into_iter().map(|p| p + base).collect()
        })
        .collect();
    Ok(ClusteredLayout {
        nodes: abs[..n].to_vec(),
        boxes: (0..k).map(|c| Rect::from_origin_size(abs[n + c], size[n + c])).collect(),
        waypoints,
        flow: top_flow,
    })
}
