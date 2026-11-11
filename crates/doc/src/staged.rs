//! A big import decoded in slices.
//!
//! Importing a snapshot into Loro is cheap (the CRDT loads its state lazily: about 8 ms for 100,000 elements), and
//! decoding every element into the view is not (about 25 us each). The first part runs inside
//! [`LoroStore::apply_remote`](crate::LoroStore::apply_remote); the second is cut into the slices of
//! [`LoroStore::import_step`](crate::LoroStore::import_step), so the caller can draw a frame between two of them.
//! Listing the ids of all elements is the one part that cannot be cut (about 1.5 us per element).

use drafftink_scene::{MemoryStore, SceneRead};
use loro::LoroValue;

use crate::error::DocError;
use crate::schema::{ROOT_ELEMENTS, ROOT_FILES};
use crate::store::{Core, ImportProgress};

/// One thing to bring into the view.
#[derive(Debug)]
pub(crate) enum Item {
    /// The document name and background.
    Meta,
    /// The element with this id (also an element only the view has, which is then removed).
    Element(String),
    /// The file with this id.
    File(String),
}

/// The work that is left of a staged import.
#[derive(Debug)]
pub(crate) struct Staged {
    items: Vec<Item>,
    next: usize,
}

impl Staged {
    /// Lists everything that the CRDT and the view hold, meta first.
    pub(crate) fn scan(view: &MemoryStore, core: &Core) -> Result<Self, DocError> {
        core.guard(|| {
            let mut items = vec![Item::Meta];
            let elements = Keys::of(&core.doc.get_map(ROOT_ELEMENTS).get_value());
            let files = Keys::of(&core.doc.get_map(ROOT_FILES).get_value());
            // What only the view has goes away.
            let gone_elements: Vec<String> =
                view.iter_all().map(|e| e.id.to_string()).filter(|id| !elements.has(id)).collect();
            let gone_files: Vec<String> =
                view.files().map(|(id, _)| id.to_string()).filter(|id| !files.has(id)).collect();
            items.extend(elements.list().map(|k| Item::Element(k.to_owned())));
            items.extend(gone_elements.into_iter().map(Item::Element));
            items.extend(files.list().map(|k| Item::File(k.to_owned())));
            items.extend(gone_files.into_iter().map(Item::File));
            Staged { items, next: 0 }
        })
    }

    pub(crate) fn progress(&self) -> ImportProgress {
        ImportProgress { done: self.next, total: self.items.len() }
    }

    /// Takes the next `max` items.
    pub(crate) fn take(&mut self, max: usize) -> &[Item] {
        let from = self.next;
        self.next = (from + max.max(1)).min(self.items.len());
        self.items.get(from..self.next).unwrap_or_default()
    }

    pub(crate) fn is_done(&self) -> bool {
        self.next >= self.items.len()
    }
}

/// The keys of a map container. `get_value` is the cheap way to list them: the child containers stay unopened (about
/// 0.8 us per key at 100,000 against 2 us for `for_each`).
struct Keys(LoroValue);

impl Keys {
    fn of(v: &LoroValue) -> Self {
        Keys(v.clone())
    }

    fn list(&self) -> impl Iterator<Item = &str> {
        let map = match &self.0 {
            LoroValue::Map(m) => Some(m),
            _ => None,
        };
        map.into_iter().flat_map(|m| m.keys().map(String::as_str))
    }

    fn has(&self, key: &str) -> bool {
        matches!(&self.0, LoroValue::Map(m) if m.contains_key(key))
    }
}
