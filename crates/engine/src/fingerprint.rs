//! A fingerprint of a document's content, kept current from the change sets of the working store.
//!
//! The tab bar shows a document as changed when its content differs from what was saved or opened. Revisions
//! cannot say that: undo and redo are tracked as aliases ([`crate::store::TabDoc::same_content`]), but a remote
//! update that reverts an edit, or the same edit made by hand again, leaves a new revision with old content. The
//! fingerprint compares the content itself.
//!
//! It is a sum of one hash per element, so adding or removing an element is one addition or subtraction and the
//! cost of keeping it current follows what changed, not the size of the scene. The hash of an element leaves out
//! its `version`, which counts local changes and is not content. Two independent 64-bit hashes make a false
//! match (a changed document shown as clean) as unlikely as 2^-128 for each pair of states.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hasher};
use std::rc::Rc;

use drafftink_scene::{ChangeSet, Element, ElementId, SceneRead};

/// Two independent 64-bit hashes that are added together, so the order of the parts does not matter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Sum(u64, u64);

impl Sum {
    fn add(self, other: Sum) -> Sum {
        Sum(self.0.wrapping_add(other.0), self.1.wrapping_add(other.1))
    }

    fn sub(self, other: Sum) -> Sum {
        Sum(self.0.wrapping_sub(other.0), self.1.wrapping_sub(other.1))
    }

    /// The two hashes of `bytes`; the second is salted so that it does not follow from the first.
    fn of(bytes: &[u8]) -> Sum {
        let (mut a, mut b) = (DefaultHasher::new(), DefaultHasher::new());
        a.write(bytes);
        b.write_u8(0xA5);
        b.write(bytes);
        Sum(a.finish(), b.finish())
    }
}

/// The content of a document: its elements (tombstones included), its name and background, and the ids of its
/// image files (the id is the hash of the bytes). Equal fingerprints mean equal content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fingerprint {
    elements: Sum,
    count: usize,
    side: Sum,
}

/// What the working store changed since the fingerprint last caught up.
#[derive(Debug, Default)]
pub(crate) struct Pending {
    ids: BTreeSet<ElementId>,
    /// The name, the background or the file table changed.
    side: bool,
}

impl Pending {
    /// Notes a commit.
    pub(crate) fn record(&mut self, changes: &ChangeSet) {
        self.ids.extend(changes.ids().cloned());
        self.side |= changes.meta || !changes.files.is_empty();
    }
}

/// The feed that the store's subscription fills and [`Tracker::print`] drains.
pub(crate) type Feed = Rc<RefCell<Pending>>;

/// The hash of each element, and the totals built from them.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    /// `None` until the first fingerprint: it is built from the whole scene once.
    per_element: Option<HashMap<ElementId, Sum>>,
    total: Sum,
    side: Option<Sum>,
}

impl Tracker {
    /// The fingerprint of `store` now. The first call reads the whole scene; later calls read only what `feed`
    /// recorded.
    pub(crate) fn print<R: SceneRead>(&mut self, store: &R, feed: &Feed) -> Fingerprint {
        let pending = std::mem::take(&mut *feed.borrow_mut());
        let mut buffer = Vec::new();
        match self.per_element.as_mut() {
            None => {
                let mut all = HashMap::with_capacity(store.len_all());
                let mut total = Sum::default();
                for element in store.iter_all() {
                    let sum = hash_element(element, &mut buffer);
                    total = total.add(sum);
                    all.insert(element.id.clone(), sum);
                }
                self.per_element = Some(all);
                self.total = total;
            }
            Some(all) => {
                for id in &pending.ids {
                    let old = match store.get(id) {
                        Some(element) => all.insert(id.clone(), hash_element(element, &mut buffer)),
                        None => all.remove(id),
                    };
                    self.total = self.total.sub(old.unwrap_or_default());
                    if let Some(new) = all.get(id) {
                        self.total = self.total.add(*new);
                    }
                }
            }
        }
        if pending.side || self.side.is_none() {
            self.side = Some(hash_side(store));
        }
        Fingerprint {
            elements: self.total,
            count: self.per_element.as_ref().map_or(0, HashMap::len),
            side: self.side.unwrap_or_default(),
        }
    }
}

/// The hash of an element's content: its JSON without `version`.
fn hash_element(element: &Element, buffer: &mut Vec<u8>) -> Sum {
    let mut content = element.clone();
    content.version = 0;
    buffer.clear();
    if serde_json::to_writer(&mut *buffer, &content).is_err() {
        // The element cannot be written as JSON (it always can); its debug text is content too.
        buffer.clear();
        buffer.extend_from_slice(format!("{content:?}").as_bytes());
    }
    Sum::of(buffer)
}

/// The hash of the name, the background and the file ids.
fn hash_side<R: SceneRead>(store: &R) -> Sum {
    let meta = store.meta();
    let mut text = format!("{}\n{:?}", meta.name, meta.background);
    for (id, _) in store.files() {
        text.push('\n');
        text.push_str(&id.to_string());
    }
    Sum::of(text.as_bytes())
}
