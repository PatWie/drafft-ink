//! `MemoryStore`: a plain in-memory [`SceneStore`] with no CRDT. For tests, for headless tools, and for library
//! users who only want to show a diagram.
//!
//! Writes go straight into the maps, and the transaction keeps a journal of each touched element's first value, so
//! reads inside the transaction see its own writes, rollback restores the journal, and commit diffs the journal
//! against the final values to build the change set.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::sync::Arc;

use crate::color::Color;
use crate::derived::DerivedIndex;
use crate::document::{DocMeta, Document, FileEntry};
use crate::element::Element;
use crate::id::{ElementId, FileId};
use crate::limits::{MAX_ELEMENTS, MAX_FILE_BYTES, MAX_FILES};
use crate::shared::SharedDocument;
use crate::store::{
    Change, ChangeKind, ChangeSet, Fields, FileRejection, SceneRead, SceneStore, SceneTx, StoreError, SubscriptionId,
    load_document,
};
use crate::validate::{is_image_media_type, is_valid_name, validate_element};
use crate::zindex::ZIndex;

type Callback = Box<dyn FnMut(&ChangeSet)>;

/// A scene held in memory.
#[derive(Default)]
pub struct MemoryStore {
    /// Shared, so that a snapshot ([`MemoryStore::share_all`]) is a list of pointers. An edit replaces the element
    /// (the snapshot keeps the old one).
    elements: HashMap<ElementId, Arc<Element>>,
    /// The same elements in z order, so that walking the scene is a walk of one tree and no lookup per element.
    order: BTreeMap<(ZIndex, ElementId), Arc<Element>>,
    live: usize,
    meta: DocMeta,
    files: BTreeMap<FileId, FileEntry>,
    derived: DerivedIndex,
    revision: u64,
    subscribers: Vec<(SubscriptionId, Callback)>,
    next_subscription: u64,
}

impl MemoryStore {
    /// An empty scene named "Untitled" with a white background.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store holding `doc`. The document is validated; one with errors is refused.
    pub fn from_document(doc: &Document) -> Result<Self, StoreError> {
        let mut store = Self::new();
        load_document(&mut store, doc)?;
        Ok(store)
    }

    fn put(&mut self, e: Arc<Element>) {
        self.order.insert((e.index.clone(), e.id.clone()), Arc::clone(&e));
        if !e.deleted {
            self.live += 1;
        }
        self.elements.insert(e.id.clone(), e);
    }

    /// Every element, tombstones included, back to front, shared with the store: no element is copied. Later edits
    /// to the store do not show in the list. This is what a long job (a write to disk) takes before it starts.
    pub fn share_all(&self) -> Vec<Arc<Element>> {
        self.order.values().map(Arc::clone).collect()
    }

    /// Sets the version of an element. The element is copied when a snapshot holds it, and then both lists of the
    /// store get the copy.
    fn set_version(&mut self, id: &ElementId, version: u64) {
        let Some(e) = self.elements.get_mut(id) else { return };
        if e.version == version {
            return;
        }
        let key = (e.index.clone(), id.clone());
        // The z list holds the element too: let go of it, so that a sole owner is changed in place.
        self.order.remove(&key);
        Arc::make_mut(e).version = version;
        self.order.insert(key, Arc::clone(e));
    }

    /// The whole content as it is now, elements shared and not copied ([`Self::share_all`]).
    pub fn share_document(&self) -> SharedDocument {
        SharedDocument {
            name: self.meta.name.clone(),
            background: self.meta.background,
            elements: self.share_all(),
            files: self.files.clone(),
        }
    }

    fn take(&mut self, id: &ElementId) -> Option<Arc<Element>> {
        let e = self.elements.remove(id)?;
        self.order.remove(&(e.index.clone(), e.id.clone()));
        if !e.deleted {
            self.live = self.live.saturating_sub(1);
        }
        Some(e)
    }
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryStore")
            .field("elements", &self.elements.len())
            .field("live", &self.live)
            .field("revision", &self.revision)
            .finish()
    }
}

impl SceneRead for MemoryStore {
    fn get(&self, id: &ElementId) -> Option<&Element> {
        self.elements.get(id).map(|e| &**e)
    }

    fn iter_all(&self) -> impl Iterator<Item = &Element> {
        self.order.values().map(|e| &**e)
    }

    fn len(&self) -> usize {
        self.live
    }

    fn len_all(&self) -> usize {
        self.elements.len()
    }

    fn meta(&self) -> &DocMeta {
        &self.meta
    }

    fn file(&self, id: &FileId) -> Option<&FileEntry> {
        self.files.get(id)
    }

    fn files(&self) -> impl Iterator<Item = (&FileId, &FileEntry)> {
        self.files.iter()
    }

    fn top_z(&self) -> Option<ZIndex> {
        self.order.last_key_value().map(|((z, _), _)| z.clone())
    }

    fn bottom_z(&self) -> Option<ZIndex> {
        self.order.first_key_value().map(|((z, _), _)| z.clone())
    }

    fn derived_index(&self) -> Option<&DerivedIndex> {
        Some(&self.derived)
    }

    fn version_stamp(&self) -> Option<u64> {
        Some(self.revision)
    }

    fn z_neighbors(&self, id: &ElementId) -> (Option<ZIndex>, Option<ZIndex>) {
        let Some(e) = self.elements.get(id).map(|e| &**e) else { return (None, None) };
        let key = (e.index.clone(), e.id.clone());
        let below =
            self.order.range((Bound::Unbounded, Bound::Excluded(&key))).next_back().map(|((z, _), _)| z.clone());
        let above = self.order.range((Bound::Excluded(&key), Bound::Unbounded)).next().map(|((z, _), _)| z.clone());
        (below, above)
    }
}

impl SceneStore for MemoryStore {
    type Tx<'a> = MemoryTx<'a>;

    fn begin(&mut self) -> MemoryTx<'_> {
        MemoryTx { store: self, touched: HashMap::new(), meta: None, files: HashMap::new(), done: false }
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn derived(&self) -> &DerivedIndex {
        &self.derived
    }

    fn subscribe(&mut self, callback: Box<dyn FnMut(&ChangeSet)>) -> SubscriptionId {
        self.next_subscription += 1;
        let id = SubscriptionId(self.next_subscription);
        self.subscribers.push((id, callback));
        id
    }

    fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        let before = self.subscribers.len();
        self.subscribers.retain(|(s, _)| *s != id);
        self.subscribers.len() != before
    }
}

/// A transaction on a [`MemoryStore`].
pub struct MemoryTx<'a> {
    store: &'a mut MemoryStore,
    /// First value of every touched element (`None`: it did not exist).
    touched: HashMap<ElementId, Option<Arc<Element>>>,
    meta: Option<DocMeta>,
    files: HashMap<FileId, Option<FileEntry>>,
    done: bool,
}

impl SceneRead for MemoryTx<'_> {
    fn get(&self, id: &ElementId) -> Option<&Element> {
        self.store.get(id)
    }
    fn iter_all(&self) -> impl Iterator<Item = &Element> {
        self.store.iter_all()
    }
    fn len(&self) -> usize {
        self.store.len()
    }
    fn len_all(&self) -> usize {
        self.store.len_all()
    }
    fn meta(&self) -> &DocMeta {
        self.store.meta()
    }
    fn file(&self, id: &FileId) -> Option<&FileEntry> {
        self.store.file(id)
    }
    fn files(&self) -> impl Iterator<Item = (&FileId, &FileEntry)> {
        self.store.files()
    }
    fn top_z(&self) -> Option<ZIndex> {
        self.store.top_z()
    }
    fn bottom_z(&self) -> Option<ZIndex> {
        self.store.bottom_z()
    }
    fn z_neighbors(&self, id: &ElementId) -> (Option<ZIndex>, Option<ZIndex>) {
        self.store.z_neighbors(id)
    }
    fn derived_index(&self) -> Option<&DerivedIndex> {
        // The index follows commits, so it is exact only while this transaction has written no element.
        self.touched.is_empty().then_some(&self.store.derived)
    }
    fn version_stamp(&self) -> Option<u64> {
        self.touched.is_empty().then_some(self.store.revision)
    }
}

impl MemoryTx<'_> {
    fn restore(&mut self) {
        for (id, orig) in std::mem::take(&mut self.touched) {
            self.store.take(&id);
            if let Some(e) = orig {
                self.store.put(e);
            }
        }
        if let Some(m) = self.meta.take() {
            self.store.meta = m;
        }
        for (id, orig) in std::mem::take(&mut self.files) {
            match orig {
                Some(f) => {
                    self.store.files.insert(id, f);
                }
                None => {
                    self.store.files.remove(&id);
                }
            }
        }
    }
}

impl Drop for MemoryTx<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.restore();
        }
    }
}

impl SceneTx for MemoryTx<'_> {
    fn insert(&mut self, mut element: Element) -> Result<(), StoreError> {
        validate_element(&element).map_err(|error| StoreError::Invalid { id: element.id.clone(), error })?;
        if self.store.elements.contains_key(&element.id) {
            return Err(StoreError::AlreadyExists(element.id));
        }
        if self.store.elements.len() >= MAX_ELEMENTS {
            return Err(StoreError::TooManyElements(MAX_ELEMENTS));
        }
        if element.version == 0 {
            element.version = 1;
        }
        self.touched.entry(element.id.clone()).or_insert(None);
        self.store.put(Arc::new(element));
        Ok(())
    }

    fn update<F: FnOnce(&mut Element)>(&mut self, id: &ElementId, f: F) -> Result<(), StoreError> {
        // Work on a copy so that a panicking or rejected closure leaves the store untouched.
        let mut work = self.store.elements.get(id).ok_or_else(|| StoreError::NotFound(id.clone()))?.as_ref().clone();
        f(&mut work);
        if work.id != *id {
            return Err(StoreError::IdChanged(id.clone()));
        }
        validate_element(&work).map_err(|error| StoreError::Invalid { id: id.clone(), error })?;
        let old = self.store.take(id).ok_or_else(|| StoreError::NotFound(id.clone()))?;
        self.touched.entry(id.clone()).or_insert(Some(old));
        self.store.put(Arc::new(work));
        Ok(())
    }

    fn remove(&mut self, id: &ElementId) -> Result<(), StoreError> {
        let old = self.store.take(id).ok_or_else(|| StoreError::NotFound(id.clone()))?;
        self.touched.entry(id.clone()).or_insert(Some(old));
        Ok(())
    }

    fn set_name(&mut self, name: &str) -> Result<(), StoreError> {
        if !is_valid_name(name) {
            return Err(StoreError::BadName);
        }
        if self.meta.is_none() {
            self.meta = Some(self.store.meta.clone());
        }
        self.store.meta.name = name.to_owned();
        Ok(())
    }

    fn set_background(&mut self, color: Color) {
        if self.meta.is_none() {
            self.meta = Some(self.store.meta.clone());
        }
        self.store.meta.background = color;
    }

    fn put_file(&mut self, entry: FileEntry) -> Result<FileId, StoreError> {
        if !is_image_media_type(&entry.mime) {
            return Err(FileRejection::MediaType(entry.mime).into());
        }
        if entry.bytes.len() > MAX_FILE_BYTES {
            return Err(FileRejection::TooLarge { len: entry.bytes.len(), max: MAX_FILE_BYTES }.into());
        }
        let id = FileId::of(&entry.bytes);
        if self.store.files.contains_key(&id) {
            return Ok(id);
        }
        if self.store.files.len() >= MAX_FILES {
            return Err(FileRejection::TooMany { max: MAX_FILES }.into());
        }
        self.files.entry(id.clone()).or_insert(None);
        self.store.files.insert(id.clone(), entry);
        Ok(id)
    }

    fn remove_file(&mut self, id: &FileId) -> bool {
        match self.store.files.remove(id) {
            Some(old) => {
                self.files.entry(id.clone()).or_insert(Some(old));
                true
            }
            None => false,
        }
    }

    fn commit(mut self) -> Result<ChangeSet, StoreError> {
        self.done = true;
        let mut elements = Vec::new();
        for (id, orig) in std::mem::take(&mut self.touched) {
            let fin = self.store.elements.get(&id);
            match (orig, fin) {
                (None, None) => {}
                (None, Some(f)) => {
                    elements.push(Change { id, kind: ChangeKind::Added, fields: Fields::all(), version: f.version });
                }
                (Some(_), None) => {
                    elements.push(Change { id, kind: ChangeKind::Removed, fields: Fields::all(), version: 0 });
                }
                (Some(o), Some(f)) => {
                    let fields = Fields::diff(&o, f);
                    let version = if fields.is_empty() { o.version } else { o.version.saturating_add(1) };
                    self.store.set_version(&id, version);
                    if !fields.is_empty() {
                        elements.push(Change { id, kind: ChangeKind::Updated, fields, version });
                    }
                }
            }
        }
        elements.sort_by(|a, b| a.id.cmp(&b.id));
        let meta = self.meta.take().is_some_and(|orig| orig != self.store.meta);
        let mut files: Vec<FileId> = std::mem::take(&mut self.files)
            .into_iter()
            .filter(|(id, orig)| orig.as_ref() != self.store.files.get(id))
            .map(|(id, _)| id)
            .collect();
        files.sort();
        let mut cs = ChangeSet { revision: 0, elements, meta, files };
        if cs.is_empty() {
            return Ok(cs);
        }
        let store = &mut *self.store;
        store.revision += 1;
        cs.revision = store.revision;
        let mut derived = std::mem::take(&mut store.derived);
        derived.apply(&cs, &*store);
        store.derived = derived;
        let mut subscribers = std::mem::take(&mut store.subscribers);
        for (_, cb) in &mut subscribers {
            cb(&cs);
        }
        // Subscribers cannot reach the store, so nothing was added meanwhile.
        store.subscribers = subscribers;
        Ok(cs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::{ElementKind, Transform};
    use crate::id::{IdSource, SeqIds};

    fn rect(ids: &mut SeqIds, z: &ZIndex) -> Element {
        let mut e = Element::new(ids.element_id(), ElementKind::Rectangle, z.clone());
        e.transform = Transform::new(0.0, 0.0, 10.0, 10.0);
        e
    }

    #[test]
    fn basic_insert_update_remove() {
        let mut s = MemoryStore::new();
        let mut ids = SeqIds::new();
        let z = ZIndex::first();
        let e = rect(&mut ids, &z);
        let id = e.id.clone();
        let mut tx = s.begin();
        tx.insert(e).unwrap();
        assert_eq!(tx.len(), 1);
        let cs = tx.commit().unwrap();
        assert_eq!(cs.revision, 1);
        assert_eq!(cs.get(&id).unwrap().kind, ChangeKind::Added);
        assert_eq!(s.get(&id).unwrap().version, 1);

        let mut tx = s.begin();
        tx.update(&id, |e| e.transform.x = 5.0).unwrap();
        let cs = tx.commit().unwrap();
        let c = cs.get(&id).unwrap();
        assert_eq!((c.kind, c.fields, c.version), (ChangeKind::Updated, Fields::POSITION, 2));

        let mut tx = s.begin();
        tx.remove(&id).unwrap();
        let cs = tx.commit().unwrap();
        assert_eq!(cs.get(&id).unwrap().kind, ChangeKind::Removed);
        assert!(s.is_empty() && s.get(&id).is_none());
    }

    #[test]
    fn dropped_transaction_rolls_back() {
        let mut s = MemoryStore::new();
        let mut ids = SeqIds::new();
        let e = rect(&mut ids, &ZIndex::first());
        let id = e.id.clone();
        {
            let mut tx = s.begin();
            tx.insert(e).unwrap();
            tx.set_name("changed").unwrap();
        }
        assert!(s.get(&id).is_none());
        assert_eq!(s.meta().name, "Untitled");
        assert_eq!(s.revision(), 0);
    }
}
