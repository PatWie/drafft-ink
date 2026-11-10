//! [`FragmentCache`]: encoded fragments by element id, bounded by bytes.
//!
//! Lookup is two-step so that the common cases cost almost nothing:
//!
//! 1. Same `(id, version)` as the entry: a hit. One hash lookup.
//! 2. A newer version: the content hash ([`crate::paint_hash`], which leaves out position and angle) decides. Equal
//!    means the element only moved or turned (or changed something that does not draw, such as its group): the
//!    entry is kept under the new version and counted as a *reuse*. Different means a resize or a restyle: the
//!    element is encoded again.
//!
//! Entries are kept per theme, so switching theme back and forth does not thrash. Memory is bounded by an
//! estimate of the encoded bytes. When the budget is passed, the least recently used entries are dropped in one
//! batch down to 90% of the budget, which keeps a hit free of any list manipulation.

use std::collections::HashMap;
use std::sync::Arc;

use drafftink_profile::{Counter, inc};
use drafftink_scene::{Element, ElementId, FileId};

use crate::fragment::Fragment;
use crate::hash::paint_hash;
use crate::theme::Theme;

/// Default byte budget of the fragment cache.
pub const DEFAULT_CACHE_BYTES: usize = 256 << 20;

/// What a lookup did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The entry had this version.
    Hit,
    /// A newer version with the same content: the fragment was reused.
    Reuse,
    /// There was no valid entry; the element was encoded.
    Miss,
}

/// Cache counters since creation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Same version.
    pub hits: u64,
    /// Newer version, same content.
    pub reuses: u64,
    /// Encoded because there was nothing valid.
    pub misses: u64,
    /// Entries dropped for the byte budget.
    pub evictions: u64,
    /// Entries now.
    pub entries: usize,
    /// Estimated bytes now.
    pub bytes: usize,
}

struct Entry {
    fragment: Arc<Fragment>,
    version: u64,
    hash: u128,
    tick: u64,
}

/// Fragments by element id and theme.
pub struct FragmentCache {
    maps: [HashMap<ElementId, Entry>; 2],
    bytes: usize,
    max_bytes: usize,
    tick: u64,
    stats: CacheStats,
}

impl Default for FragmentCache {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_BYTES)
    }
}

impl FragmentCache {
    /// A cache holding at most about `max_bytes` of encoded scenes.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            maps: [HashMap::new(), HashMap::new()],
            bytes: 0,
            max_bytes: max_bytes.max(1),
            tick: 0,
            stats: CacheStats::default(),
        }
    }

    /// Counters.
    pub fn stats(&self) -> CacheStats {
        CacheStats { entries: self.len(), bytes: self.bytes, ..self.stats }
    }

    /// Entries held, both themes.
    pub fn len(&self) -> usize {
        self.maps[0].len() + self.maps[1].len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Changes the byte budget and evicts if needed.
    pub fn set_max_bytes(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes.max(1);
        self.evict();
    }

    /// Drops every entry (a different document was loaded: versions of the old one mean nothing).
    pub fn clear(&mut self) {
        self.maps[0].clear();
        self.maps[1].clear();
        self.bytes = 0;
    }

    /// Drops the entries of one element.
    pub fn remove(&mut self, id: &ElementId) {
        for m in &mut self.maps {
            if let Some(e) = m.remove(id) {
                self.bytes = self.bytes.saturating_sub(e.fragment.bytes);
            }
        }
    }

    /// Drops the entries of every element in `ids`.
    pub fn remove_all<'a>(&mut self, ids: impl IntoIterator<Item = &'a ElementId>) {
        for id in ids {
            self.remove(id);
        }
    }

    /// The picture a held fragment is waiting for, if its element shows a placeholder.
    pub fn awaiting(&self, id: &ElementId, theme: Theme) -> Option<&FileId> {
        self.maps[theme.index()].get(id).and_then(|e| e.fragment.awaiting_image.as_ref())
    }

    /// Whether a fragment is held for `id` and `theme`, without counting a use.
    pub fn contains(&self, id: &ElementId, theme: Theme) -> bool {
        self.maps[theme.index()].contains_key(id)
    }

    /// The fragment for `e`: from the cache when valid, else `paint` is called and its result stored.
    ///
    /// `stale` is true when a held fragment must be encoded again regardless of its version (a placeholder whose
    /// picture has arrived; see [`Self::awaiting`]).
    pub fn get_or_paint(
        &mut self,
        e: &Element,
        theme: Theme,
        stale: bool,
        paint: impl FnOnce() -> Fragment,
    ) -> (Arc<Fragment>, Outcome) {
        self.tick += 1;
        let tick = self.tick;
        let map = &mut self.maps[theme.index()];
        let mut new_hash = None;
        if let Some(entry) = map.get_mut(&e.id)
            && !stale
        {
            if entry.version == e.version && e.version != 0 {
                entry.tick = tick;
                self.stats.hits += 1;
                inc(Counter::CacheHit);
                return (Arc::clone(&entry.fragment), Outcome::Hit);
            }
            let h = paint_hash(e);
            if h == entry.hash {
                entry.version = e.version;
                entry.tick = tick;
                self.stats.reuses += 1;
                inc(Counter::CacheHit);
                return (Arc::clone(&entry.fragment), Outcome::Reuse);
            }
            new_hash = Some(h);
        }
        self.stats.misses += 1;
        inc(Counter::CacheMiss);
        let fragment = Arc::new(paint());
        let hash = new_hash.unwrap_or_else(|| paint_hash(e));
        let bytes = fragment.bytes;
        let entry = Entry { fragment: Arc::clone(&fragment), version: e.version, hash, tick };
        if let Some(old) = self.maps[theme.index()].insert(e.id.clone(), entry) {
            self.bytes = self.bytes.saturating_sub(old.fragment.bytes);
        }
        self.bytes += bytes;
        if self.bytes > self.max_bytes {
            self.evict();
        }
        (fragment, Outcome::Miss)
    }

    fn evict(&mut self) {
        if self.bytes <= self.max_bytes {
            return;
        }
        let target = self.max_bytes - self.max_bytes / 10;
        let mut order: Vec<(u64, usize, ElementId)> = Vec::with_capacity(self.len());
        for (t, m) in self.maps.iter().enumerate() {
            order.extend(m.iter().map(|(id, e)| (e.tick, t, id.clone())));
        }
        order.sort_unstable_by_key(|(tick, _, _)| *tick);
        // The most recently used entry stays even if it alone is over budget.
        order.pop();
        for (_, t, id) in order {
            if self.bytes <= target {
                break;
            }
            if let Some(e) = self.maps[t].remove(&id) {
                self.bytes = self.bytes.saturating_sub(e.fragment.bytes);
                self.stats.evictions += 1;
            }
        }
    }
}
