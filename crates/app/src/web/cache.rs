//! The in-memory half of the web storage: every entry of the four namespaces, plus the writes that
//! IndexedDB has not confirmed yet. Pure data, no browser types, so the rules are unit-tested natively.

use std::collections::BTreeMap;

use drafftink_engine::ports::{Ns, StoredInfo};

/// One stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The content.
    /// Shared between the cache and the write that is waiting for the database (a document is up to 45 MB).
    pub bytes: std::sync::Arc<Vec<u8>>,
    /// Last write, milliseconds since the Unix epoch.
    pub modified_ms: u64,
}

/// Where an entry lives: namespace and key.
pub(crate) type Slot = (Ns, String);

/// A write waiting for IndexedDB: `Some` stores the entry, `None` deletes the key.
pub(crate) type Pending = BTreeMap<Slot, Option<Entry>>;

/// The cache. Reads are answered from `entries`; every change also lands in `pending`, where a later write
/// to the same key replaces an earlier one, so a burst of autosaves is one IndexedDB write.
#[derive(Debug, Default)]
pub(crate) struct Cache {
    entries: BTreeMap<Slot, Entry>,
    pending: Pending,
}

impl Cache {
    /// Fills the cache from what IndexedDB held. Nothing becomes pending.
    pub(crate) fn load(&mut self, ns: Ns, key: String, entry: Entry) {
        self.entries.insert((ns, key), entry);
    }

    pub(crate) fn get(&self, ns: Ns, key: &str) -> Option<&Entry> {
        self.entries.get(&(ns, key.to_owned()))
    }

    pub(crate) fn put(&mut self, ns: Ns, key: &str, bytes: &[u8], now_ms: u64) {
        self.put_owned(ns, key, bytes.to_vec(), now_ms);
    }

    /// Like [`Self::put`] for bytes the caller gives up: they are not copied.
    pub(crate) fn put_owned(&mut self, ns: Ns, key: &str, bytes: Vec<u8>, now_ms: u64) {
        let entry = Entry { bytes: std::sync::Arc::new(bytes), modified_ms: now_ms };
        self.entries.insert((ns, key.to_owned()), entry.clone());
        self.pending.insert((ns, key.to_owned()), Some(entry));
    }

    pub(crate) fn delete(&mut self, ns: Ns, key: &str) {
        let slot = (ns, key.to_owned());
        if self.entries.remove(&slot).is_some() || self.pending.contains_key(&slot) {
            self.pending.insert(slot, None);
        }
    }

    /// The entries of one namespace, sorted by name.
    pub(crate) fn list(&self, ns: Ns) -> Vec<StoredInfo> {
        self.entries
            .iter()
            .filter(|((n, _), _)| *n == ns)
            .map(|((_, key), e)| StoredInfo {
                name: key.clone(),
                size: e.bytes.len() as u64,
                modified_ms: e.modified_ms,
            })
            .collect()
    }

    /// True when writes wait for IndexedDB.
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Takes every waiting write, to hand to one IndexedDB transaction.
    pub(crate) fn take_pending(&mut self) -> Pending {
        std::mem::take(&mut self.pending)
    }

    /// Puts back writes whose transaction failed. A newer write to the same key wins.
    pub(crate) fn requeue(&mut self, failed: Pending) {
        for (slot, write) in failed {
            self.pending.entry(slot).or_insert(write);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(c: &Cache, ns: Ns) -> Vec<String> {
        c.list(ns).into_iter().map(|i| i.name).collect()
    }

    #[test]
    fn put_get_list_delete() {
        let mut c = Cache::default();
        c.put(Ns::Documents, "b", b"22", 5);
        c.put(Ns::Documents, "a", b"1", 6);
        c.put(Ns::Autosave, "x", b"", 7);
        assert_eq!(names(&c, Ns::Documents), ["a", "b"]);
        assert_eq!(c.get(Ns::Documents, "b").map(|e| e.bytes.to_vec()), Some(b"22".to_vec()));
        assert_eq!(c.list(Ns::Documents)[1].size, 2);
        assert_eq!(c.list(Ns::Documents)[0].modified_ms, 6);
        c.delete(Ns::Documents, "a");
        assert_eq!(names(&c, Ns::Documents), ["b"]);
        assert!(c.get(Ns::Documents, "a").is_none());
        assert!(c.list(Ns::Settings).is_empty());
    }

    #[test]
    fn loaded_entries_are_not_pending() {
        let mut c = Cache::default();
        c.load(Ns::Settings, "s".into(), Entry { bytes: std::sync::Arc::new(vec![1]), modified_ms: 9 });
        assert!(!c.has_pending());
        assert_eq!(c.get(Ns::Settings, "s").map(|e| e.modified_ms), Some(9));
    }

    #[test]
    fn a_burst_of_writes_to_one_key_is_one_pending_write() {
        let mut c = Cache::default();
        for i in 0..10u8 {
            c.put(Ns::Autosave, "slot", &[i], u64::from(i));
        }
        let batch = c.take_pending();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch.values().next().and_then(|w| w.as_ref().map(|e| e.bytes.to_vec())), Some(vec![9]));
        assert!(!c.has_pending());
    }

    #[test]
    fn deleting_a_key_that_never_reached_the_database_still_cancels_the_write() {
        let mut c = Cache::default();
        c.put(Ns::Documents, "tmp", b"x", 1);
        c.delete(Ns::Documents, "tmp");
        let batch = c.take_pending();
        assert_eq!(batch.get(&(Ns::Documents, "tmp".to_owned())), Some(&None));
    }

    #[test]
    fn deleting_a_missing_key_does_nothing() {
        let mut c = Cache::default();
        c.delete(Ns::Documents, "nope");
        assert!(!c.has_pending());
    }

    #[test]
    fn a_failed_batch_is_requeued_but_never_over_a_newer_write() {
        let mut c = Cache::default();
        c.put(Ns::Documents, "a", b"old", 1);
        c.put(Ns::Documents, "b", b"keep", 1);
        let failed = c.take_pending();
        c.put(Ns::Documents, "a", b"new", 2);
        c.requeue(failed);
        let batch = c.take_pending();
        let bytes =
            |k: &str| batch.get(&(Ns::Documents, k.to_owned())).and_then(|w| w.as_ref().map(|e| e.bytes.to_vec()));
        assert_eq!(bytes("a"), Some(b"new".to_vec()));
        assert_eq!(bytes("b"), Some(b"keep".to_vec()));
    }
}
