//! [`SpatialIndex`]: an R-tree from element id to bounds.
//!
//! It answers "which ids are near this rect or point" in logarithmic time and keeps one entry per
//! id. It stores bounds only; exact hit tests on the candidates are the caller's job (see
//! [`crate::Outline`]). Result order is unspecified: sort by z-order afterwards.
//!
//! Non-finite bounds are rejected on insert. A query with a non-finite rect or point returns
//! nothing. Bounds are normalised (`x0 <= x1`) on the way in.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

use kurbo::{Point, Rect};
use rstar::{AABB, PointDistance, RTree, RTreeObject};

use crate::aabb;

/// Why an index operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexError {
    /// The bounds contain NaN or an infinity.
    InvalidBounds,
    /// `update` was called for an id that is not in the index.
    NotFound,
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexError::InvalidBounds => f.write_str("bounds must be finite numbers"),
            IndexError::NotFound => f.write_str("no entry with that id; use insert to add it"),
        }
    }
}

impl std::error::Error for IndexError {}

#[derive(Debug, Clone, PartialEq)]
struct Entry<Id> {
    id: Id,
    env: AABB<[f64; 2]>,
}

impl<Id> RTreeObject for Entry<Id> {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.env
    }
}

impl<Id> PointDistance for Entry<Id> {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        self.env.distance_2(point)
    }
}

fn to_env(r: &Rect) -> AABB<[f64; 2]> {
    AABB::from_corners([r.x0, r.y0], [r.x1, r.y1])
}

fn from_env(e: &AABB<[f64; 2]>) -> Rect {
    let (l, u) = (e.lower(), e.upper());
    Rect::new(l[0], l[1], u[0], u[1])
}

/// An R-tree keyed by id.
#[derive(Debug, Clone)]
pub struct SpatialIndex<Id> {
    tree: RTree<Entry<Id>>,
    bounds: HashMap<Id, Rect>,
}

impl<Id> Default for SpatialIndex<Id> {
    fn default() -> Self {
        Self { tree: RTree::new(), bounds: HashMap::new() }
    }
}

impl<Id: Clone + Eq + Hash> SpatialIndex<Id> {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build an index from many entries at once. Much faster than repeated `insert` for large
    /// scenes. Entries with invalid bounds are skipped; a repeated id keeps its last bounds.
    pub fn bulk_load<I: IntoIterator<Item = (Id, Rect)>>(entries: I) -> Self {
        let mut bounds = HashMap::new();
        for (id, r) in entries {
            if aabb::is_finite(&r) {
                bounds.insert(id, aabb::normalize(r));
            }
        }
        let items: Vec<Entry<Id>> = bounds.iter().map(|(id, r)| Entry { id: id.clone(), env: to_env(r) }).collect();
        Self { tree: RTree::bulk_load(items), bounds }
    }

    /// Replace the content with `entries` (see [`SpatialIndex::bulk_load`]).
    pub fn rebuild<I: IntoIterator<Item = (Id, Rect)>>(&mut self, entries: I) {
        *self = Self::bulk_load(entries);
    }

    /// Number of ids in the index.
    pub fn len(&self) -> usize {
        self.bounds.len()
    }

    /// True when the index holds no id.
    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }

    /// Remove every entry.
    pub fn clear(&mut self) {
        self.tree = RTree::new();
        self.bounds.clear();
    }

    /// True when `id` is in the index.
    pub fn contains(&self, id: &Id) -> bool {
        self.bounds.contains_key(id)
    }

    /// The stored bounds of an id.
    pub fn get(&self, id: &Id) -> Option<Rect> {
        self.bounds.get(id).copied()
    }

    /// Add an id, or move it when it is already present. Returns the previous bounds.
    /// On error the index is unchanged.
    pub fn insert(&mut self, id: Id, bounds: Rect) -> Result<Option<Rect>, IndexError> {
        if !aabb::is_finite(&bounds) {
            return Err(IndexError::InvalidBounds);
        }
        let bounds = aabb::normalize(bounds);
        let previous = self.bounds.insert(id.clone(), bounds);
        if let Some(old) = previous {
            if old == bounds {
                return Ok(Some(old));
            }
            self.tree.remove(&Entry { id: id.clone(), env: to_env(&old) });
        }
        self.tree.insert(Entry { id, env: to_env(&bounds) });
        Ok(previous)
    }

    /// Change the bounds of an existing id. Fails with [`IndexError::NotFound`] for an unknown id.
    pub fn update(&mut self, id: &Id, bounds: Rect) -> Result<Rect, IndexError> {
        if !aabb::is_finite(&bounds) {
            return Err(IndexError::InvalidBounds);
        }
        if !self.bounds.contains_key(id) {
            return Err(IndexError::NotFound);
        }
        match self.insert(id.clone(), bounds)? {
            Some(old) => Ok(old),
            None => Err(IndexError::NotFound),
        }
    }

    /// Remove an id. Returns its bounds when it was present.
    pub fn remove(&mut self, id: &Id) -> Option<Rect> {
        let old = self.bounds.remove(id)?;
        self.tree.remove(&Entry { id: id.clone(), env: to_env(&old) });
        Some(old)
    }

    /// The union of all bounds, or `None` when the index is empty.
    pub fn total_bounds(&self) -> Option<Rect> {
        if self.is_empty() {
            return None;
        }
        Some(from_env(&self.tree.root().envelope()))
    }

    /// Every entry, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (&Id, Rect)> + '_ {
        self.bounds.iter().map(|(id, r)| (id, *r))
    }

    /// Entries whose bounds overlap or touch `rect`.
    pub fn query_rect(&self, rect: &Rect) -> impl Iterator<Item = (&Id, Rect)> + '_ {
        let env = aabb::is_finite(rect).then(|| to_env(&aabb::normalize(*rect)));
        env.map(|e| self.tree.locate_in_envelope_intersecting(e))
            .into_iter()
            .flatten()
            .map(|e| (&e.id, from_env(&e.env)))
    }

    /// Entries whose bounds lie completely inside `rect` (marquee that must enclose).
    pub fn query_rect_contained(&self, rect: &Rect) -> impl Iterator<Item = (&Id, Rect)> + '_ {
        let env = aabb::is_finite(rect).then(|| to_env(&aabb::normalize(*rect)));
        env.map(|e| self.tree.locate_in_envelope(e)).into_iter().flatten().map(|e| (&e.id, from_env(&e.env)))
    }

    /// Entries whose bounds are within `tolerance` of the point (0 means the point is inside the
    /// bounds). The distance is Euclidean to the bounds rect.
    pub fn query_point(&self, p: Point, tolerance: f64) -> impl Iterator<Item = (&Id, Rect)> + '_ {
        let tol = if tolerance.is_finite() { tolerance.max(0.0) } else { 0.0 };
        let ok = crate::point_is_finite(p);
        ok.then(|| self.tree.locate_within_distance([p.x, p.y], tol * tol))
            .into_iter()
            .flatten()
            .map(|e| (&e.id, from_env(&e.env)))
    }

    /// The entry whose bounds are nearest to the point, with the distance to its bounds.
    pub fn nearest(&self, p: Point) -> Option<(&Id, Rect, f64)> {
        self.nearest_k(p, 1).into_iter().next()
    }

    /// The `k` entries nearest to the point, closest first.
    pub fn nearest_k(&self, p: Point, k: usize) -> Vec<(&Id, Rect, f64)> {
        if !crate::point_is_finite(p) || k == 0 {
            return Vec::new();
        }
        self.tree
            .nearest_neighbor_iter_with_distance_2([p.x, p.y])
            .take(k)
            .map(|(e, d2)| (&e.id, from_env(&e.env), d2.sqrt()))
            .collect()
    }

    /// Entries within `max_distance` of the point, closest first.
    pub fn nearest_within(&self, p: Point, max_distance: f64) -> Vec<(&Id, Rect, f64)> {
        if !crate::point_is_finite(p) || !max_distance.is_finite() || max_distance < 0.0 {
            return Vec::new();
        }
        let limit = max_distance * max_distance;
        self.tree
            .nearest_neighbor_iter_with_distance_2([p.x, p.y])
            .take_while(|(_, d2)| *d2 <= limit)
            .map(|(e, d2)| (&e.id, from_env(&e.env), d2.sqrt()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, x + w, y + h)
    }

    fn ids<'a>(it: impl Iterator<Item = (&'a u32, Rect)>) -> Vec<u32> {
        let mut v: Vec<u32> = it.map(|(i, _)| *i).collect();
        v.sort_unstable();
        v
    }

    fn sample() -> SpatialIndex<u32> {
        let mut ix = SpatialIndex::new();
        ix.insert(1, r(0.0, 0.0, 10.0, 10.0)).unwrap();
        ix.insert(2, r(20.0, 0.0, 10.0, 10.0)).unwrap();
        ix.insert(3, r(0.0, 20.0, 10.0, 10.0)).unwrap();
        ix
    }

    #[test]
    fn insert_get_remove() {
        let mut ix = sample();
        assert_eq!(ix.len(), 3);
        assert_eq!(ix.get(&2), Some(r(20.0, 0.0, 10.0, 10.0)));
        assert_eq!(ix.remove(&2), Some(r(20.0, 0.0, 10.0, 10.0)));
        assert_eq!(ix.remove(&2), None);
        assert!(!ix.contains(&2) && ix.len() == 2);
        assert!(ids(ix.query_rect(&r(-100.0, -100.0, 500.0, 500.0))) == vec![1, 3]);
    }

    #[test]
    fn insert_replaces_and_update_moves() {
        let mut ix = sample();
        assert_eq!(ix.insert(1, r(100.0, 100.0, 5.0, 5.0)).unwrap(), Some(r(0.0, 0.0, 10.0, 10.0)));
        assert_eq!(ix.len(), 3);
        assert!(ids(ix.query_point(Point::new(5.0, 5.0), 0.0)).is_empty(), "old position is gone");
        assert_eq!(ids(ix.query_point(Point::new(102.0, 102.0), 0.0)), vec![1]);
        assert_eq!(ix.update(&3, r(50.0, 50.0, 1.0, 1.0)).unwrap(), r(0.0, 20.0, 10.0, 10.0));
        assert_eq!(ix.update(&99, r(0.0, 0.0, 1.0, 1.0)), Err(IndexError::NotFound));
        assert_eq!(ids(ix.query_point(Point::new(50.5, 50.5), 0.0)), vec![3]);
    }

    #[test]
    fn same_bounds_update_is_a_no_op() {
        let mut ix = sample();
        ix.insert(1, r(0.0, 0.0, 10.0, 10.0)).unwrap();
        assert_eq!(ix.len(), 3);
        assert_eq!(ids(ix.query_point(Point::new(5.0, 5.0), 0.0)), vec![1]);
    }

    #[test]
    fn rect_queries() {
        let ix = sample();
        assert_eq!(ids(ix.query_rect(&r(5.0, 5.0, 20.0, 1.0))), vec![1, 2]);
        assert_eq!(ids(ix.query_rect(&r(10.0, 0.0, 10.0, 10.0))), vec![1, 2], "touching counts");
        assert_eq!(ids(ix.query_rect_contained(&r(-1.0, -1.0, 12.0, 40.0))), vec![1, 3]);
        assert!(ids(ix.query_rect_contained(&r(1.0, 1.0, 5.0, 5.0))).is_empty());
    }

    #[test]
    fn point_queries_with_tolerance() {
        let ix = sample();
        assert_eq!(ids(ix.query_point(Point::new(12.0, 5.0), 0.0)), Vec::<u32>::new());
        assert_eq!(ids(ix.query_point(Point::new(12.0, 5.0), 2.0)), vec![1]);
        assert_eq!(ids(ix.query_point(Point::new(15.0, 5.0), 5.0)), vec![1, 2]);
        // circle, not square: the corner of a tolerance square is out of reach
        assert!(ids(ix.query_point(Point::new(13.0, 13.0), 4.0)).is_empty());
        assert_eq!(ids(ix.query_point(Point::new(13.0, 13.0), 4.3)), vec![1]);
    }

    #[test]
    fn nearest_queries() {
        let ix = sample();
        let (id, _, d) = ix.nearest(Point::new(14.0, 5.0)).unwrap();
        assert_eq!((*id, d), (1, 4.0));
        let k = ix.nearest_k(Point::new(14.0, 5.0), 2);
        assert_eq!(k.iter().map(|x| *x.0).collect::<Vec<_>>(), vec![1, 2]);
        assert!(k[0].2 <= k[1].2);
        assert_eq!(ix.nearest_within(Point::new(14.0, 5.0), 6.0).len(), 2);
        assert_eq!(ix.nearest_within(Point::new(14.0, 5.0), 5.0).len(), 1);
        assert!(SpatialIndex::<u32>::new().nearest(Point::ZERO).is_none());
    }

    #[test]
    fn bulk_load_matches_inserts() {
        let entries: Vec<(u32, Rect)> =
            (0..200).map(|i| (i, r((i % 20) as f64 * 15.0, (i / 20) as f64 * 15.0, 10.0, 10.0))).collect();
        let a = SpatialIndex::bulk_load(entries.clone());
        let mut b = SpatialIndex::new();
        for (i, rc) in &entries {
            b.insert(*i, *rc).unwrap();
        }
        let q = r(30.0, 30.0, 100.0, 50.0);
        assert_eq!(ids(a.query_rect(&q)), ids(b.query_rect(&q)));
        assert_eq!(a.len(), 200);
        assert_eq!(a.total_bounds(), b.total_bounds());
    }

    #[test]
    fn bulk_load_skips_bad_and_keeps_last_duplicate() {
        let ix = SpatialIndex::bulk_load([
            (1u32, r(0.0, 0.0, 1.0, 1.0)),
            (2, Rect::new(f64::NAN, 0.0, 1.0, 1.0)),
            (1, r(5.0, 5.0, 1.0, 1.0)),
        ]);
        assert_eq!(ix.len(), 1);
        assert_eq!(ix.get(&1), Some(r(5.0, 5.0, 1.0, 1.0)));
    }

    #[test]
    fn hostile_input() {
        let mut ix = sample();
        assert_eq!(ix.insert(9, Rect::new(0.0, 0.0, f64::INFINITY, 1.0)), Err(IndexError::InvalidBounds));
        assert_eq!(ix.update(&1, Rect::new(f64::NAN, 0.0, 1.0, 1.0)), Err(IndexError::InvalidBounds));
        assert_eq!(ix.get(&1), Some(r(0.0, 0.0, 10.0, 10.0)), "failed update leaves the entry alone");
        assert_eq!(ix.query_rect(&Rect::new(f64::NAN, 0.0, 1.0, 1.0)).count(), 0);
        assert_eq!(ix.query_point(Point::new(f64::NAN, 0.0), 1.0).count(), 0);
        assert_eq!(ix.query_point(Point::new(5.0, 5.0), f64::NAN).count(), 1, "bad tolerance counts as 0");
        assert!(ix.nearest(Point::new(f64::INFINITY, 0.0)).is_none());
        assert!(ix.nearest_k(Point::ZERO, 0).is_empty());
        assert!(ix.nearest_within(Point::ZERO, -1.0).is_empty());
    }

    #[test]
    fn inverted_bounds_are_normalised() {
        let mut ix = SpatialIndex::new();
        ix.insert(1u32, Rect::new(10.0, 10.0, 0.0, 0.0)).unwrap();
        assert_eq!(ix.get(&1), Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
        assert_eq!(ix.query_point(Point::new(5.0, 5.0), 0.0).count(), 1);
    }

    #[test]
    fn many_updates_stay_consistent() {
        let mut ix = SpatialIndex::new();
        for i in 0..500u32 {
            ix.insert(i, r(i as f64 * 3.0, 0.0, 2.0, 2.0)).unwrap();
        }
        for round in 0..3 {
            for i in 0..500u32 {
                ix.update(&i, r(i as f64 * 3.0, (round + 1) as f64 * 10.0, 2.0, 2.0)).unwrap();
            }
        }
        assert_eq!(ix.len(), 500);
        assert_eq!(ix.query_rect(&r(-1.0, -1.0, 5000.0, 5.0)).count(), 0);
        assert_eq!(ix.query_rect(&r(-1.0, 29.0, 5000.0, 5.0)).count(), 500);
        for i in (0..500u32).step_by(2) {
            ix.remove(&i);
        }
        assert_eq!(ix.query_rect(&r(-1.0, -1.0, 5000.0, 5000.0)).count(), 250);
    }
}
