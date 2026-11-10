//! The `SceneStore` contract: read access, transactional writes, change sets, notifications.
//!
//! The same contract is implemented by [`crate::memory::MemoryStore`] (plain in-memory, no CRDT) and by the
//! Loro-backed store in the `doc` crate. [`crate::conformance`] holds the tests both must pass.
//!
//! What a store guarantees:
//!
//! * Elements are ordered by `(index, id)`. [`SceneRead::iter`] yields live elements back to front; tombstones
//!   (`deleted`) take part in the order and show up in [`SceneRead::iter_all`] only.
//! * Writes happen inside a transaction. Nothing is visible to other readers until [`SceneTx::commit`]; dropping a
//!   transaction without committing undoes it. Reads inside a transaction see its own writes.
//! * A commit returns one [`ChangeSet`] with an entry per touched element, saying which fields changed. Elements
//!   whose final value equals their first value are not reported, and a commit that changes nothing is a no-op: no
//!   revision, no notification.
//! * Every reported change of an existing element raises its `version` by exactly one, so `(id, version)` is a
//!   valid cache key.
//! * Subscribers are called once per commit, after the store is consistent, in subscription order.

use std::collections::BTreeSet;
use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use crate::color::Color;
use crate::derived::DerivedIndex;
use crate::document::{DocMeta, Document, FileEntry};
use crate::element::Element;
use crate::id::{ElementId, FileId};
use crate::validate::{ElementError, Report};
use crate::zindex::ZIndex;

/// The groups of fields of an element, as a set. A [`Change`] carries the fields that differ.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Fields(u16);

impl Fields {
    /// `transform.x` or `transform.y`.
    pub const POSITION: Fields = Fields(1);
    /// `transform.w` or `transform.h`.
    pub const SIZE: Fields = Fields(1 << 1);
    /// `transform.angle`.
    pub const ANGLE: Fields = Fields(1 << 2);
    /// `transform.flip_x` or `flip_y`.
    pub const FLIP: Fields = Fields(1 << 3);
    /// The kind or any data inside it (points, text, bindings, ...).
    pub const KIND: Fields = Fields(1 << 4);
    /// `style`.
    pub const STYLE: Fields = Fields(1 << 5);
    /// `index` (z-order).
    pub const INDEX: Fields = Fields(1 << 6);
    /// `group_ids`.
    pub const GROUPS: Fields = Fields(1 << 7);
    /// `frame_id`.
    pub const FRAME: Fields = Fields(1 << 8);
    /// `locked`.
    pub const LOCKED: Fields = Fields(1 << 9);
    /// `deleted` (the tombstone flag).
    pub const DELETED: Fields = Fields(1 << 10);
    /// `name`.
    pub const NAME: Fields = Fields(1 << 11);
    /// `link`.
    pub const LINK: Fields = Fields(1 << 12);
    /// `custom`.
    pub const CUSTOM: Fields = Fields(1 << 13);

    const ALL_BITS: u16 = (1 << 14) - 1;
    const NAMES: [(Fields, &'static str); 14] = [
        (Fields::POSITION, "position"),
        (Fields::SIZE, "size"),
        (Fields::ANGLE, "angle"),
        (Fields::FLIP, "flip"),
        (Fields::KIND, "kind"),
        (Fields::STYLE, "style"),
        (Fields::INDEX, "index"),
        (Fields::GROUPS, "groups"),
        (Fields::FRAME, "frame"),
        (Fields::LOCKED, "locked"),
        (Fields::DELETED, "deleted"),
        (Fields::NAME, "name"),
        (Fields::LINK, "link"),
        (Fields::CUSTOM, "custom"),
    ];

    /// No field.
    pub const fn empty() -> Fields {
        Fields(0)
    }

    /// Every field (what an added or removed element reports).
    pub const fn all() -> Fields {
        Fields(Self::ALL_BITS)
    }

    /// True when no field is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when every field of `other` is set.
    pub const fn contains(self, other: Fields) -> bool {
        self.0 & other.0 == other.0
    }

    /// True when at least one field of `other` is set.
    pub const fn intersects(self, other: Fields) -> bool {
        self.0 & other.0 != 0
    }

    /// The fields in which `a` and `b` differ. `version` is not a field.
    pub fn diff(a: &Element, b: &Element) -> Fields {
        let mut f = Fields::empty();
        let (ta, tb) = (&a.transform, &b.transform);
        if ta.x != tb.x || ta.y != tb.y {
            f |= Fields::POSITION;
        }
        if ta.w != tb.w || ta.h != tb.h {
            f |= Fields::SIZE;
        }
        if ta.angle != tb.angle {
            f |= Fields::ANGLE;
        }
        if ta.flip_x != tb.flip_x || ta.flip_y != tb.flip_y {
            f |= Fields::FLIP;
        }
        if a.kind != b.kind {
            f |= Fields::KIND;
        }
        if a.style != b.style {
            f |= Fields::STYLE;
        }
        if a.index != b.index {
            f |= Fields::INDEX;
        }
        if a.group_ids != b.group_ids {
            f |= Fields::GROUPS;
        }
        if a.frame_id != b.frame_id {
            f |= Fields::FRAME;
        }
        if a.locked != b.locked {
            f |= Fields::LOCKED;
        }
        if a.deleted != b.deleted {
            f |= Fields::DELETED;
        }
        if a.name != b.name {
            f |= Fields::NAME;
        }
        if a.link != b.link {
            f |= Fields::LINK;
        }
        if a.custom != b.custom {
            f |= Fields::CUSTOM;
        }
        f
    }

    /// True when the change can alter the pixels of the element (so its cached drawing is stale).
    pub fn affects_paint(self) -> bool {
        self.intersects(Fields(
            Self::POSITION.0
                | Self::SIZE.0
                | Self::ANGLE.0
                | Self::FLIP.0
                | Self::KIND.0
                | Self::STYLE.0
                | Self::DELETED.0
                | Self::INDEX.0,
        ))
    }

    /// True when the change can alter the element's outline (so hit-test and spatial index entries are stale).
    /// Moving alone does not change the local geometry but does change where the element is.
    pub fn affects_geometry(self) -> bool {
        self.intersects(Fields(
            Self::POSITION.0 | Self::SIZE.0 | Self::ANGLE.0 | Self::FLIP.0 | Self::KIND.0 | Self::DELETED.0
                | Self::STYLE.0,
        ))
    }

    /// The names of the set fields.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES.iter().filter(|(f, _)| self.contains(*f)).map(|(_, n)| *n).collect()
    }
}

impl BitOr for Fields {
    type Output = Fields;
    fn bitor(self, rhs: Fields) -> Fields {
        Fields(self.0 | rhs.0)
    }
}

impl BitOrAssign for Fields {
    fn bitor_assign(&mut self, rhs: Fields) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for Fields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fields({})", self.names().join("|"))
    }
}

/// What happened to an element in one commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// The id did not exist before the commit.
    Added,
    /// The element existed before and after, and differs.
    Updated,
    /// The id existed before and is gone (a hard removal; a tombstone is an update of `deleted`).
    Removed,
}

/// One element's net change in a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The element.
    pub id: ElementId,
    /// Added, updated or removed.
    pub kind: ChangeKind,
    /// The fields that differ between the value before the commit and after it (all fields for add and remove).
    pub fields: Fields,
    /// The element's version after the commit, 0 when removed.
    pub version: u64,
}

/// The net effect of one commit, or of one batch of remote updates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeSet {
    /// The store revision after the commit. 0 for an empty change set.
    pub revision: u64,
    /// Changed elements, sorted by id.
    pub elements: Vec<Change>,
    /// Name or background changed.
    pub meta: bool,
    /// Blobs added or removed, sorted.
    pub files: Vec<FileId>,
}

impl ChangeSet {
    /// True when nothing changed.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty() && !self.meta && self.files.is_empty()
    }

    /// The change of one element, if it was touched.
    pub fn get(&self, id: &ElementId) -> Option<&Change> {
        self.elements.binary_search_by(|c| c.id.cmp(id)).ok().and_then(|i| self.elements.get(i))
    }

    /// Ids of every touched element.
    pub fn ids(&self) -> impl Iterator<Item = &ElementId> {
        self.elements.iter().map(|c| &c.id)
    }

    /// True when some element's z-order key changed or an element was added or removed.
    pub fn order_changed(&self) -> bool {
        self.elements.iter().any(|c| c.kind != ChangeKind::Updated || c.fields.intersects(Fields::INDEX))
    }
}

/// Why a store operation was refused. The store is unchanged after an error from `insert`, `update`, `remove`,
/// `put_file` or `set_name`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StoreError {
    /// No element has this id.
    #[error("no element has the id {0}")]
    NotFound(ElementId),
    /// An element (possibly deleted) already has this id.
    #[error("an element with the id {0} already exists; use a fresh id")]
    AlreadyExists(ElementId),
    /// The element breaks a rule on its own.
    #[error("element {id} is invalid: {error}")]
    Invalid {
        /// The element.
        id: ElementId,
        /// The broken rule.
        error: ElementError,
    },
    /// An update changed the element's id.
    #[error("an update cannot change the id {0}")]
    IdChanged(ElementId),
    /// The scene already holds the most elements it may.
    #[error("the scene is full: at most {0} elements")]
    TooManyElements(usize),
    /// A blob or the table is over its limit.
    #[error("the file is rejected: {0}")]
    BadFile(String),
    /// The name is empty, too long or has control characters.
    #[error("the document name must be 1 to {} characters without control characters", crate::limits::MAX_NAME_CHARS)]
    BadName,
    /// A whole document failed validation before loading.
    #[error("the document breaks {} rule(s)", .0.errors().count())]
    InvalidDocument(Report),
    /// The store's backend failed (for the Loro store: the CRDT layer).
    #[error("store backend failed: {0}")]
    Backend(String),
}

/// Handle returned by [`SceneStore::subscribe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionId(pub u64);

/// Read access to a scene, shared by stores and open transactions.
pub trait SceneRead {
    /// The element with this id, deleted or not.
    fn get(&self, id: &ElementId) -> Option<&Element>;

    /// All elements including tombstones, back to front.
    fn iter_all(&self) -> impl Iterator<Item = &Element>;

    /// Number of live elements.
    fn len(&self) -> usize;

    /// Number of elements including tombstones.
    fn len_all(&self) -> usize;

    /// Name and background.
    fn meta(&self) -> &DocMeta;

    /// An image blob.
    fn file(&self, id: &FileId) -> Option<&FileEntry>;

    /// All image blobs, sorted by id.
    fn files(&self) -> impl Iterator<Item = (&FileId, &FileEntry)>;

    /// The key of the topmost element (tombstones included), if any.
    fn top_z(&self) -> Option<ZIndex>;

    /// The key of the bottom element (tombstones included), if any.
    fn bottom_z(&self) -> Option<ZIndex>;

    /// The keys of the elements directly below and above `id` (tombstones included), for making a key between.
    fn z_neighbors(&self, id: &ElementId) -> (Option<ZIndex>, Option<ZIndex>);

    /// Live elements, back to front.
    fn iter(&self) -> impl Iterator<Item = &Element> {
        self.iter_all().filter(|e| !e.deleted)
    }

    /// True when there are no live elements.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True when an element (live or deleted) has this id.
    fn contains(&self, id: &ElementId) -> bool {
        self.get(id).is_some()
    }

    /// A key for a new element above everything.
    fn next_top_index(&self) -> Result<ZIndex, crate::zindex::IndexError> {
        ZIndex::between(self.top_z().as_ref(), None)
    }
}

/// A scene that can be changed in transactions.
pub trait SceneStore: SceneRead {
    /// The transaction type.
    type Tx<'a>: SceneTx
    where
        Self: 'a;

    /// Opens a transaction. Only one can be open at a time (the borrow checker enforces it).
    fn begin(&mut self) -> Self::Tx<'_>;

    /// Number of non-empty commits so far. Raised by local commits and by applied remote changes.
    fn revision(&self) -> u64;

    /// The derived indexes, kept up to date on every commit. They reflect committed state only.
    fn derived(&self) -> &DerivedIndex;

    /// Registers a callback that receives the change set of every commit.
    fn subscribe(&mut self, callback: Box<dyn FnMut(&ChangeSet)>) -> SubscriptionId;

    /// Removes a callback. Returns false when it was not registered.
    fn unsubscribe(&mut self, id: SubscriptionId) -> bool;
}

/// An open transaction. Dropping it without calling [`SceneTx::commit`] discards its writes.
pub trait SceneTx: SceneRead {
    /// Adds an element. Its `version` becomes 1 when it is 0.
    fn insert(&mut self, element: Element) -> Result<(), StoreError>;

    /// Changes an element in place. The closure must not change the id. The element is checked with
    /// `validate_element` afterwards; on failure it keeps its old value.
    fn update<F: FnOnce(&mut Element)>(&mut self, id: &ElementId, f: F) -> Result<(), StoreError>;

    /// Removes an element for good. Commands delete by setting `deleted`; hard removal is for garbage collection
    /// and for undoing a creation.
    fn remove(&mut self, id: &ElementId) -> Result<(), StoreError>;

    /// Sets the document name.
    fn set_name(&mut self, name: &str) -> Result<(), StoreError>;

    /// Sets the background colour.
    fn set_background(&mut self, color: Color);

    /// Stores a blob and returns its id (the SHA-256 of the bytes). Storing the same bytes again is a no-op.
    fn put_file(&mut self, entry: FileEntry) -> Result<FileId, StoreError>;

    /// Removes a blob. Returns false when there was none.
    fn remove_file(&mut self, id: &FileId) -> bool;

    /// Makes the writes visible and returns what changed.
    fn commit(self) -> Result<ChangeSet, StoreError>
    where
        Self: Sized;

    /// Discards the writes. Same as dropping the transaction.
    fn rollback(self)
    where
        Self: Sized,
    {
    }
}

/// Copies a store's content into a [`Document`], elements back to front.
pub fn snapshot<R: SceneRead>(r: &R) -> Document {
    let meta = r.meta();
    Document {
        name: meta.name.clone(),
        background: meta.background,
        elements: r.iter_all().cloned().collect(),
        files: r.files().map(|(id, f)| (id.clone(), f.clone())).collect(),
        ..Document::default()
    }
}

/// Replaces the whole content of `store` with `doc` in one transaction. The document is validated first; a document
/// with errors is refused and the store is unchanged.
pub fn load_document<S: SceneStore>(store: &mut S, doc: &Document) -> Result<ChangeSet, StoreError> {
    let report = doc.validate();
    if report.has_errors() {
        return Err(StoreError::InvalidDocument(report));
    }
    let existing: Vec<ElementId> = store.iter_all().map(|e| e.id.clone()).collect();
    let old_files: Vec<FileId> = store.files().map(|(id, _)| id.clone()).collect();
    let keep: BTreeSet<&FileId> = doc.files.keys().collect();
    let mut tx = store.begin();
    for id in &existing {
        tx.remove(id)?;
    }
    for id in old_files.iter().filter(|id| !keep.contains(id)) {
        tx.remove_file(id);
    }
    tx.set_name(&doc.name)?;
    tx.set_background(doc.background);
    for entry in doc.files.values() {
        tx.put_file(entry.clone())?;
    }
    for e in &doc.elements {
        tx.insert(e.clone())?;
    }
    tx.commit()
}
