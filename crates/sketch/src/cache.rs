//! A small bounded cache for sketch geometry, keyed by [`SketchKey`].
//!
//! Two generations: new and recently used entries live in `current`; when it is full it becomes
//! `previous` and the old `previous` is dropped. A hit in `previous` moves the entry back to
//! `current`. So memory is bounded by twice the capacity, a hot entry survives indefinitely, and
//! there is never a wholesale flush (the old app cleared its whole cache at 500 entries).
//!
//! The cache counts hits and misses. A miss is a regeneration; callers forward the count to the
//! `sketch_regens` perf counter.

use std::collections::HashMap;
use std::sync::Arc;

use crate::input::SketchInput;
use crate::key::{SketchKey, sketch_key};
use crate::output::SketchGeometry;
use crate::sketch::sketch;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    /// Number of times the geometry was generated.
    pub misses: u64,
}

#[derive(Debug)]
pub struct SketchCache {
    current: HashMap<SketchKey, Arc<SketchGeometry>>,
    previous: HashMap<SketchKey, Arc<SketchGeometry>>,
    capacity: usize,
    stats: CacheStats,
}

impl SketchCache {
    /// Entries per generation. At least 1.
    pub fn new(capacity: usize) -> Self {
        Self {
            current: HashMap::new(),
            previous: HashMap::new(),
            capacity: capacity.max(1),
            stats: CacheStats::default(),
        }
    }

    /// The geometry for `input`, generated on a miss.
    pub fn get(&mut self, input: &SketchInput) -> Arc<SketchGeometry> {
        self.get_with_key(sketch_key(input), input)
    }

    /// Like [`get`](Self::get) with a key the caller already has (memoised per element version).
    /// The key must be `sketch_key(input)`.
    pub fn get_with_key(&mut self, key: SketchKey, input: &SketchInput) -> Arc<SketchGeometry> {
        if let Some(g) = self.current.get(&key) {
            self.stats.hits += 1;
            return Arc::clone(g);
        }
        if let Some(g) = self.previous.remove(&key) {
            self.stats.hits += 1;
            self.insert(key, Arc::clone(&g));
            return g;
        }
        self.stats.misses += 1;
        let g = Arc::new(sketch(input));
        self.insert(key, Arc::clone(&g));
        g
    }

    fn insert(&mut self, key: SketchKey, g: Arc<SketchGeometry>) {
        if self.current.len() >= self.capacity {
            self.previous = std::mem::take(&mut self.current);
        }
        self.current.insert(key, g);
    }

    pub fn len(&self) -> usize {
        self.current.len() + self.previous.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    pub fn clear(&mut self) {
        self.current.clear();
        self.previous.clear();
    }
}

impl Default for SketchCache {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Roundness, SketchParams, SketchShape};

    fn rect(w: f64, seed: u32) -> SketchInput {
        SketchInput::new(
            SketchShape::Rect { width: w, height: 40.0, roundness: Roundness::Sharp },
            SketchParams { seed, ..SketchParams::default() },
        )
    }

    #[test]
    fn second_lookup_hits_and_shares_the_allocation() {
        let mut c = SketchCache::new(8);
        let a = c.get(&rect(100.0, 1));
        let b = c.get(&rect(100.0, 1));
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(c.stats(), CacheStats { hits: 1, misses: 1 });
        c.get(&rect(101.0, 1));
        assert_eq!(c.stats().misses, 2);
    }

    #[test]
    fn memory_is_bounded_and_hot_entries_survive() {
        let mut c = SketchCache::new(4);
        let hot = rect(50.0, 1);
        for i in 0..100 {
            c.get(&hot);
            c.get(&rect(100.0 + f64::from(i), 1));
            assert!(c.len() <= 8, "{}", c.len());
        }
        // 100 cold misses plus the first miss of the hot entry; every other hot lookup hit.
        assert_eq!(c.stats().misses, 101);
        assert_eq!(c.stats().hits, 99);
    }
}
