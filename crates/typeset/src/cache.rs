//! A bounded least-recently-used cache with an entry limit and a weight (bytes) limit.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

struct Slot<V> {
    value: V,
    weight: usize,
    tick: u64,
}

/// Least-recently-used cache. `get` and `insert` are `O(log n)`.
pub(crate) struct Lru<K, V> {
    map: HashMap<K, Slot<V>>,
    order: BTreeMap<u64, K>,
    tick: u64,
    weight: usize,
    max_entries: usize,
    max_weight: usize,
    evictions: u64,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    pub fn new(max_entries: usize, max_weight: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            weight: 0,
            max_entries: max_entries.max(1),
            max_weight: max_weight.max(1),
            evictions: 0,
        }
    }

    pub fn get(&mut self, key: &K) -> Option<V> {
        self.tick += 1;
        let tick = self.tick;
        let slot = self.map.get_mut(key)?;
        self.order.remove(&slot.tick);
        slot.tick = tick;
        self.order.insert(tick, key.clone());
        Some(slot.value.clone())
    }

    pub fn insert(&mut self, key: K, value: V, weight: usize) {
        self.remove(&key);
        self.tick += 1;
        self.order.insert(self.tick, key.clone());
        self.weight += weight;
        self.map.insert(key, Slot { value, weight, tick: self.tick });
        // Keep the newest entry even when it alone exceeds the weight limit.
        while self.map.len() > 1 && (self.map.len() > self.max_entries || self.weight > self.max_weight) {
            let Some((&oldest, _)) = self.order.iter().next() else { break };
            if let Some(k) = self.order.remove(&oldest)
                && let Some(s) = self.map.remove(&k)
            {
                self.weight = self.weight.saturating_sub(s.weight);
                self.evictions += 1;
            }
        }
    }

    fn remove(&mut self, key: &K) {
        if let Some(s) = self.map.remove(key) {
            self.order.remove(&s.tick);
            self.weight = self.weight.saturating_sub(s.weight);
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.weight = 0;
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn weight(&self) -> usize {
        self.weight
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}
