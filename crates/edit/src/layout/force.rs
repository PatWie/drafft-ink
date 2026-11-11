//! Force-directed layout (Fruchterman and Reingold), then overlap removal.
//!
//! The start positions are a golden-angle spiral by node index, so the result depends only on the input. After the
//! simulation, overlapping boxes are pushed apart. If that does not finish, the whole layout is stretched until no
//! two boxes overlap, so the result never has overlapping nodes.

use drafftink_geom::kurbo::{Point, Rect, Size};

use super::{LayoutGraph, LayoutOpts, LayoutResult};

fn rect_of(centre: Point, s: Size, pad: f64) -> Rect {
    Rect::from_center_size(centre, Size::new(s.width + pad, s.height + pad))
}

/// True when any two boxes (grown by `pad` in total) overlap. Sweep over x, so it is fast on sparse layouts.
pub(super) fn any_overlap(centres: &[Point], sizes: &[Size], pad: f64) -> bool {
    let mut idx: Vec<usize> = (0..centres.len()).collect();
    idx.sort_by(|&a, &b| (centres[a].x - sizes[a].width / 2.0).total_cmp(&(centres[b].x - sizes[b].width / 2.0)));
    for (k, &i) in idx.iter().enumerate() {
        let ri = rect_of(centres[i], sizes[i], pad);
        for &j in &idx[k + 1..] {
            let rj = rect_of(centres[j], sizes[j], pad);
            if rj.x0 >= ri.x1 {
                break;
            }
            if ri.x0 < rj.x1 && rj.x0 < ri.x1 && ri.y0 < rj.y1 && rj.y0 < ri.y1 {
                return true;
            }
        }
    }
    false
}

/// One pass of pushing overlapping pairs apart along the axis of least penetration.
fn separate_once(c: &mut [Point], sizes: &[Size], pad: f64) -> bool {
    let n = c.len();
    let mut moved = false;
    for i in 0..n {
        for j in i + 1..n {
            let dx = c[j].x - c[i].x;
            let dy = c[j].y - c[i].y;
            let px = (sizes[i].width + sizes[j].width) / 2.0 + pad - dx.abs();
            let py = (sizes[i].height + sizes[j].height) / 2.0 + pad - dy.abs();
            if px <= 0.0 || py <= 0.0 {
                continue;
            }
            moved = true;
            // A tiny extra push so that boxes that only touch afterwards do not count as overlapping.
            if px <= py {
                let s = if dx > 0.0 || (dx == 0.0 && (i + j) % 2 == 0) { 1.0 } else { -1.0 };
                c[i].x -= s * (px / 2.0 + 1e-6);
                c[j].x += s * (px / 2.0 + 1e-6);
            } else {
                let s = if dy > 0.0 || (dy == 0.0 && (i + j) % 2 == 0) { 1.0 } else { -1.0 };
                c[i].y -= s * (py / 2.0 + 1e-6);
                c[j].y += s * (py / 2.0 + 1e-6);
            }
        }
    }
    moved
}

pub(super) fn force(g: &LayoutGraph, o: &LayoutOpts) -> LayoutResult {
    let n = g.sizes.len();
    let pad = o.node_gap;
    if n == 0 {
        return LayoutResult { positions: Vec::new(), waypoints: vec![Vec::new(); g.edges.len()], flow: None };
    }
    let mean_diag = g.sizes.iter().map(|s| s.width.hypot(s.height)).sum::<f64>() / n as f64;
    let k = (mean_diag + pad + o.layer_gap / 2.0).max(1.0);

    // Golden-angle spiral start.
    let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let mut c: Vec<Point> = (0..n)
        .map(|i| {
            let r = k * 0.6 * ((i as f64) + 0.5).sqrt();
            let a = golden * i as f64;
            Point::new(r * a.cos(), r * a.sin())
        })
        .collect();

    let edges: Vec<(usize, usize)> = {
        let mut e: Vec<(usize, usize)> =
            g.edges.iter().map(|&(a, b)| if a <= b { (a, b) } else { (b, a) }).filter(|(a, b)| a != b).collect();
        e.sort_unstable();
        e.dedup();
        e
    };
    let iterations = (40_000_000 / (n * n + 1)).clamp(20, 300);
    let mut temp = k * 2.0;
    let cool = temp / (iterations as f64 + 1.0);
    let mut disp = vec![(0.0f64, 0.0f64); n];
    for _ in 0..iterations {
        disp.iter_mut().for_each(|d| *d = (0.0, 0.0));
        for i in 0..n {
            for j in i + 1..n {
                let (dx, dy) = (c[i].x - c[j].x, c[i].y - c[j].y);
                let d = dx.hypot(dy).max(0.01);
                let f = k * k / d;
                let (fx, fy) = (dx / d * f, dy / d * f);
                disp[i].0 += fx;
                disp[i].1 += fy;
                disp[j].0 -= fx;
                disp[j].1 -= fy;
            }
        }
        for &(a, b) in &edges {
            let (dx, dy) = (c[a].x - c[b].x, c[a].y - c[b].y);
            let d = dx.hypot(dy).max(0.01);
            let f = d * d / k;
            let (fx, fy) = (dx / d * f, dy / d * f);
            disp[a].0 -= fx;
            disp[a].1 -= fy;
            disp[b].0 += fx;
            disp[b].1 += fy;
        }
        for i in 0..n {
            let (dx, dy) = disp[i];
            let d = dx.hypot(dy);
            if d > 0.0 {
                let step = d.min(temp);
                c[i].x += dx / d * step;
                c[i].y += dy / d * step;
            }
        }
        temp = (temp - cool).max(0.0);
    }

    // Overlap removal.
    let passes = (20_000_000 / (n * n + 1)).clamp(5, 200);
    for _ in 0..passes {
        if !separate_once(&mut c, &g.sizes, pad) {
            break;
        }
    }
    // Last resort: stretch from the centroid until the boxes are apart. Identical centres cannot be separated by
    // stretching, so spread those first.
    if any_overlap(&c, &g.sizes, pad) {
        for (i, p) in c.iter_mut().enumerate() {
            p.x += (i as f64) * 1e-3;
            p.y += ((i * 7 % 13) as f64) * 1e-3;
        }
        let centroid =
            Point::new(c.iter().map(|p| p.x).sum::<f64>() / n as f64, c.iter().map(|p| p.y).sum::<f64>() / n as f64);
        let base = c.clone();
        let mut scale = 1.0;
        for _ in 0..80 {
            scale *= 1.3;
            for (p, b) in c.iter_mut().zip(&base) {
                *p = centroid + (*b - centroid) * scale;
            }
            if !any_overlap(&c, &g.sizes, pad) {
                break;
            }
        }
    }

    let positions =
        c.iter().zip(&g.sizes).map(|(p, s)| Point::new(p.x - s.width / 2.0, p.y - s.height / 2.0)).collect();
    LayoutResult { positions, waypoints: vec![Vec::new(); g.edges.len()], flow: None }
}
