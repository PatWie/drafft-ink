//! `library`: the shape library (docs/commands.md §2.18).
//!
//! Items are saved groups of elements for reuse. They are kept in sections: one per imported library, named
//! after the file, and one for the person's own items. The library lives in the `Storage` library namespace,
//! is not part of any document, and has no undo. It reads and writes `.excalidrawlib` (v1 and v2). The
//! file format is `drafftink-io`, taking an item out of a scene and putting it in is
//! `drafftink_edit::library`; this module keeps the list and the storage layout ([`store`]).

mod store;

pub(crate) use self::store::revision;

use std::collections::HashSet;
use std::sync::Arc;

use drafftink_edit::library as frag;
use drafftink_io::excalidraw::{self, LibraryItem};
use drafftink_io::limits::MAX_LIBRARY_ITEMS;
use drafftink_io::{FormatName, IoError, native};
use drafftink_scene::ElementId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use self::store::{Home, Item, Library, MINE_ID, MINE_NAME, Section};
use super::doc::export::{Dest, ExportBytes, PngOpts, render_png};
use super::doc::files::{self, Loaded, Provenance, Source, base64_of};
use super::doc::open::{merge_into_active, open_in_new_tab};
use crate::args;
use crate::cmd::tab::normalize_name;
use crate::cmd::view::Pt;
use crate::ports::{FileRef, PortKind};
use crate::refusal::{Refusal, echo};
use crate::session::{Ref, Session};
use crate::spec::{Host, HostEffect};
use crate::spec::{
    Batch, CommandSpec, Facts, Gate, Mutates, PortSet, Redraw, RingMode, Selection, TextGate, Typed, Undo,
};

/// What `library.list` and `library.add` say about an item.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ItemInfo {
    /// The id to pass as `item`.
    pub id: String,
    /// The name, if it has one.
    pub name: Option<String>,
    /// Visible elements in the item.
    pub element_count: usize,
    /// Bounds `[x, y, w, h]` of the item (it starts at 0, 0).
    pub bounds: Option<[f64; 4]>,
    /// When it was added, milliseconds since the Unix epoch.
    pub created: u64,
    /// The section the item belongs to: `mine` for the person's own items, else the id of an imported library.
    pub section: String,
    /// A fingerprint of what the item draws (16 hex digits). It changes when the drawing does, and not when the
    /// item is renamed, so a picture of the item stays good for as long as this stays the same.
    pub content: String,
}

fn info(i: &Item) -> ItemInfo {
    ItemInfo {
        id: i.id.clone(),
        name: i.name.clone(),
        element_count: i.document.elements.iter().filter(|e| !e.deleted).count(),
        bounds: frag::bounds(&i.document).map(|r| [round2(r.x0), round2(r.y0), round2(r.width()), round2(r.height())]),
        created: i.created_ms,
        section: i.home.id().to_owned(),
        content: format!("{:016x}", i.hash),
    }
}

/// Whose items a section holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    /// The person's own items. It has the fixed id `mine`, cannot be renamed, and is listed when it has items.
    Mine,
    /// A library that was imported.
    Imported,
}

/// What `library.list` and `library.rename_section` say about a section.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SectionInfo {
    /// The id to pass as `section`, and the `section` of its items.
    pub id: String,
    /// The name: the file an imported library came from, or `My items`.
    pub name: String,
    /// Whose items it holds.
    pub kind: SectionKind,
    /// Items in it (all of them, whatever `search` says).
    pub items: usize,
}

/// The sections that have items, the person's own first, then the imported ones in the order of their import.
fn section_infos(lib: &Library) -> Vec<SectionInfo> {
    let count = |home: &Home| lib.items.iter().filter(|i| &i.home == home).count();
    let mine = count(&Home::Mine);
    let own = (mine > 0).then(|| SectionInfo { id: MINE_ID.to_owned(), name: MINE_NAME.to_owned(), kind: SectionKind::Mine, items: mine });
    let imported = lib.sections.iter().map(|s| section_info(s, count(&Home::Section(s.id.clone()))));
    own.into_iter().chain(imported).collect()
}

fn section_info(s: &Section, items: usize) -> SectionInfo {
    SectionInfo { id: s.id.clone(), name: s.name.clone(), kind: SectionKind::Imported, items }
}

fn round2(v: f64) -> f64 {
    if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 }
}

fn no_such_item(id: &str, item: &str) -> Refusal {
    Refusal::custom(
        "no_such_item",
        id,
        format!("There is no library item `{}`. List items with `library.list`.", echo(item)),
    )
    .suggesting("library.list", None)
}

fn no_such_section(id: &str, section: &str) -> Refusal {
    Refusal::custom(
        "no_such_section",
        id,
        format!("There is no library section `{}`. List sections with `library.list`.", echo(section)),
    )
    .suggesting("library.list", None)
}

/// Finds a section by id, else by name. `mine` and `My items` are the person's own items. Two sections with the
/// same name are ambiguous.
fn find_section(id: &str, sections: &[Section], wanted: &str) -> Result<Home, Refusal> {
    if wanted == MINE_ID || wanted == MINE_NAME {
        return Ok(Home::Mine);
    }
    if let Some(s) = sections.iter().find(|s| s.id == wanted) {
        return Ok(Home::Section(s.id.clone()));
    }
    let mut named = sections.iter().filter(|s| s.name == wanted);
    match (named.next(), named.next()) {
        (None, _) => Err(no_such_section(id, wanted)),
        (Some(one), None) => Ok(Home::Section(one.id.clone())),
        (Some(_), Some(_)) => Err(Refusal::bad_arg(
            id,
            "section",
            format!("several sections are named `{}`. Pass the section id from `library.list`", echo(wanted)),
        )),
    }
}

/// Finds an item by id, else by name. Two items with the same name are ambiguous.
fn find(id: &str, items: &[Arc<Item>], wanted: &str) -> Result<usize, Refusal> {
    if let Some(i) = items.iter().position(|i| i.id == wanted) {
        return Ok(i);
    }
    let named: Vec<usize> = items.iter().enumerate().filter(|(_, i)| i.name.as_deref() == Some(wanted)).map(|(n, _)| n).collect();
    match named.as_slice() {
        [] => Err(no_such_item(id, wanted)),
        [one] => Ok(*one),
        _ => Err(Refusal::bad_arg(
            id,
            "item",
            format!("several items are named `{}`. Pass the item id from `library.list`", echo(wanted)),
        )),
    }
}

/// The state check of every library command.
fn needs_storage(s: &Session, id: &str) -> Result<(), Refusal> {
    if s.has_port(PortKind::Storage) {
        Ok(())
    } else {
        Err(Refusal::port_unavailable(id, "persistent storage", "storage port missing", "The shape library is kept in storage."))
    }
}

// ---- library.add ----

/// Arguments of `library.add`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AddArgs {
    /// The elements. Default: the selection.
    pub ids: Option<Vec<Ref>>,
    /// A name for the item (1 to 256 characters).
    pub name: Option<String>,
}

/// Output of `library.add`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AddOut {
    /// True.
    pub changed: bool,
    /// The new item.
    pub item: ItemInfo,
}

fn add(s: &mut Session, a: AddArgs) -> Result<AddOut, Refusal> {
    let id = s.cmd();
    let name = match a.name.as_deref() {
        Some(n) => Some(normalize_name(id, n)?),
        None => None,
    };
    store::writable(s, id)?;
    let ids = s.targets(a.ids.as_deref())?;
    let members = frag::members(s.store(), &ids).map_err(|e| Refusal::internal(id, e.to_string()))?;
    let keep: HashSet<ElementId> = members.into_iter().collect();
    let mut doc = native::subset(&s.tab().doc.snapshot(), &keep);
    if !frag::normalize(&mut doc) {
        return Err(Refusal::nothing_to_do(id, "add", "the elements have no size or are deleted"));
    }
    let mut lib = store::library(s);
    if lib.items.len() >= MAX_LIBRARY_ITEMS {
        return Err(Refusal::limit_exceeded(id, lib.items.len() + 1, "library items", MAX_LIBRARY_ITEMS));
    }
    let item = Arc::new(Item {
        id: store::fresh_id(s, &lib.items),
        name,
        created_ms: s.now_ms(),
        hash: frag::content_hash(&doc),
        document: doc,
        home: Home::Mine,
    });
    let out = AddOut { changed: true, item: info(&item) };
    let touched = vec![item.id.clone()];
    lib.items.push(item);
    store::flush_notes(s);
    store::commit(s, lib, &touched, &[])?;
    Ok(out)
}

// ---- library.insert ----

/// Arguments of `library.insert`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertArgs {
    /// The item id or name.
    pub item: String,
    /// The world point to centre the item on. Default: the centre of the viewport.
    #[serde(default)]
    pub at: Option<Pt>,
    /// Select the inserted elements. Default true.
    #[serde(default = "yes")]
    pub select: bool,
}

fn yes() -> bool {
    true
}

/// Output of `library.insert`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InsertOut {
    /// True.
    pub changed: bool,
    /// The item that was inserted.
    pub item: String,
    /// The new elements, back to front.
    pub ids: Vec<String>,
}

fn insert(s: &mut Session, a: InsertArgs) -> Result<InsertOut, Refusal> {
    let id = s.cmd();
    let items = store::items(s);
    let it = items.get(find(id, &items, &a.item)?).ok_or_else(|| no_such_item(id, &a.item))?;
    let at = a.at.unwrap_or_else(|| {
        let v = s.view();
        [v.scroll.x, v.scroll.y]
    });
    let r = merge_into_active(s, id, &it.document, Some(at), a.select)?;
    store::flush_notes(s);
    Ok(InsertOut { changed: true, item: it.id.clone(), ids: r.inserted.iter().map(|i| i.as_str().to_owned()).collect() })
}

// ---- library.remove ----

/// Arguments of `library.remove`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct RemoveArgs {
    /// Item ids or names to remove. Give either this or `all`.
    pub items: Option<Vec<String>>,
    /// Remove every item. The only way to empty the library.
    pub all: bool,
}

/// Output of `library.remove`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RemoveOut {
    /// False when there was nothing to remove.
    pub changed: bool,
    /// The ids that were removed.
    pub removed: Vec<String>,
}

fn remove(s: &mut Session, a: RemoveArgs) -> Result<RemoveOut, Refusal> {
    let id = s.cmd();
    if a.all == a.items.is_some() {
        return Err(Refusal::bad_args(id, "Give either `items` or all=true."));
    }
    store::writable(s, id)?;
    let Library { items, sections } = store::library(s);
    let gone: Vec<String> = if a.all {
        items.iter().map(|i| i.id.clone()).collect()
    } else {
        let mut v: Vec<String> = Vec::new();
        for w in a.items.iter().flatten() {
            let n = find(id, &items, w)?;
            if let Some(i) = items.get(n)
                && !v.contains(&i.id)
            {
                v.push(i.id.clone());
            }
        }
        v
    };
    if gone.is_empty() {
        return Ok(RemoveOut { changed: false, removed: gone });
    }
    let kept: Vec<Arc<Item>> = items.into_iter().filter(|i| !gone.contains(&i.id)).collect();
    store::flush_notes(s);
    store::commit(s, Library { items: kept, sections }, &[], &gone)?;
    Ok(RemoveOut { changed: true, removed: gone })
}

// ---- library.remove_section ----

/// Arguments of `library.remove_section`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveSectionArgs {
    /// The section id or name (`mine` for the person's own items).
    pub section: String,
}

/// Output of `library.remove_section`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RemoveSectionOut {
    /// False when the section had no items.
    pub changed: bool,
    /// The id of the section.
    pub section: String,
    /// The ids of the items that were removed with it.
    pub removed: Vec<String>,
}

fn remove_section(s: &mut Session, a: RemoveSectionArgs) -> Result<RemoveSectionOut, Refusal> {
    let id = s.cmd();
    store::writable(s, id)?;
    let Library { items, sections } = store::library(s);
    let home = find_section(id, &sections, &a.section)?;
    let (gone, kept): (Vec<Arc<Item>>, Vec<Arc<Item>>) = items.into_iter().partition(|i| i.home == home);
    let removed: Vec<String> = gone.iter().map(|i| i.id.clone()).collect();
    if removed.is_empty() {
        return Ok(RemoveSectionOut { changed: false, section: home.id().to_owned(), removed });
    }
    store::flush_notes(s);
    store::commit(s, Library { items: kept, sections }, &[], &removed)?;
    Ok(RemoveSectionOut { changed: true, section: home.id().to_owned(), removed })
}

// ---- library.rename ----

/// Arguments of `library.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameArgs {
    /// The item id or name.
    pub item: String,
    /// The new name (1 to 256 characters).
    pub name: String,
}

/// Output of `library.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RenameOut {
    /// False when the item already had this name.
    pub changed: bool,
    /// The item after the command.
    pub item: ItemInfo,
}

fn rename(s: &mut Session, a: RenameArgs) -> Result<RenameOut, Refusal> {
    let id = s.cmd();
    let name = normalize_name(id, &a.name)?;
    store::writable(s, id)?;
    let mut lib = store::library(s);
    let n = find(id, &lib.items, &a.item)?;
    let Some(slot) = lib.items.get_mut(n) else { return Err(no_such_item(id, &a.item)) };
    if slot.name.as_deref() == Some(name.as_str()) {
        return Ok(RenameOut { changed: false, item: info(slot) });
    }
    let it = Arc::make_mut(slot);
    it.name = Some(name);
    let (touched, item) = (vec![it.id.clone()], info(it));
    store::flush_notes(s);
    store::commit(s, lib, &touched, &[])?;
    Ok(RenameOut { changed: true, item })
}

// ---- library.rename_section ----

/// Arguments of `library.rename_section`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameSectionArgs {
    /// The section id or name.
    pub section: String,
    /// The new name (1 to 256 characters).
    pub name: String,
}

/// Output of `library.rename_section`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RenameSectionOut {
    /// False when the section already had this name.
    pub changed: bool,
    /// The section after the command.
    pub section: SectionInfo,
}

fn rename_section(s: &mut Session, a: RenameSectionArgs) -> Result<RenameSectionOut, Refusal> {
    let id = s.cmd();
    let name = normalize_name(id, &a.name)?;
    store::writable(s, id)?;
    let mut lib = store::library(s);
    let Home::Section(target) = find_section(id, &lib.sections, &a.section)? else {
        return Err(Refusal::bad_arg(
            id,
            "section",
            format!("`{MINE_NAME}` holds your own items and keeps its name. Rename an imported library instead"),
        ));
    };
    let items = lib.items.iter().filter(|i| i.home.id() == target).count();
    let Some(section) = lib.sections.iter_mut().find(|sec| sec.id == target) else {
        return Err(no_such_section(id, &a.section));
    };
    if section.name == name {
        return Ok(RenameSectionOut { changed: false, section: section_info(section, items) });
    }
    section.name = name;
    let out = RenameSectionOut { changed: true, section: section_info(section, items) };
    store::flush_notes(s);
    store::commit(s, lib, &[], &[])?;
    Ok(out)
}

// ---- library.list ----

/// Arguments of `library.list`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ListArgs {
    /// Only items whose name or id contains this text (case does not matter).
    pub search: Option<String>,
    /// Only the items of this section (id or name).
    pub section: Option<String>,
}

/// Output of `library.list`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ListOut {
    /// Always false.
    pub changed: bool,
    /// The items, in library order.
    pub items: Vec<ItemInfo>,
    /// The sections that have items, the person's own first. Every item's `section` is the id of one of them.
    pub sections: Vec<SectionInfo>,
}

fn list(s: &mut Session, a: ListArgs) -> Result<ListOut, Refusal> {
    let lib = store::library(s);
    store::flush_notes(s);
    let q = a.search.as_deref().map(str::to_lowercase);
    let only = a.section.as_deref().map(|w| find_section(s.cmd(), &lib.sections, w)).transpose()?;
    let items = lib
        .items
        .iter()
        .filter(|i| only.as_ref().is_none_or(|home| &i.home == home))
        .filter(|i| match &q {
            Some(q) => i.id.to_lowercase().contains(q) || i.name.as_deref().is_some_and(|n| n.to_lowercase().contains(q)),
            None => true,
        })
        .map(|i| info(i))
        .collect();
    Ok(ListOut { changed: false, items, sections: section_infos(&lib) })
}

// ---- library.thumbnail ----

/// Smallest and largest `max_side` of `library.thumbnail`, in pixels.
const THUMB_SIDE: std::ops::RangeInclusive<u32> = 16..=1024;
/// `max_side` when the caller gives none.
const THUMB_DEFAULT_SIDE: u32 = 256;
/// The largest enlargement of a small item: a 10 unit dot is not drawn 200 times as large.
const THUMB_MAX_SCALE: f64 = 8.0;

/// Arguments of `library.thumbnail`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThumbnailArgs {
    /// The item id or name.
    pub item: String,
    /// Pixels along the longer side of the picture (16 to 1024). Default 256.
    #[serde(default)]
    pub max_side: Option<u32>,
    /// Draw with the dark palette, as the canvas does in the dark theme. Default false.
    #[serde(default)]
    pub dark: bool,
}

/// Output of `library.thumbnail`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ThumbnailOut {
    /// Always false.
    pub changed: bool,
    /// The item that was drawn.
    pub item: String,
    /// Width of the picture in pixels.
    pub width: u32,
    /// Height of the picture in pixels.
    pub height: u32,
    /// The PNG, with a transparent background.
    pub bytes: ExportBytes,
}

fn thumbnail(s: &mut Session, a: ThumbnailArgs) -> Result<ThumbnailOut, Refusal> {
    let id = s.cmd();
    let side = a.max_side.unwrap_or(THUMB_DEFAULT_SIDE);
    args::in_range(id, "max_side", f64::from(side), f64::from(*THUMB_SIDE.start()), f64::from(*THUMB_SIDE.end()))?;
    let items = store::items(s);
    store::flush_notes(s);
    let it = items.get(find(id, &items, &a.item)?).ok_or_else(|| no_such_item(id, &a.item))?;
    let bounds = frag::bounds(&it.document)
        .ok_or_else(|| Refusal::nothing_to_do(id, "draw", "the item has nothing visible in it"))?;
    let longest = bounds.width().max(bounds.height()).max(1.0);
    // A little air around the drawing, so that strokes at its edge are not cut.
    let padding = (longest * 0.05).max(4.0);
    let opts = PngOpts {
        scale: (f64::from(side) / (longest + 2.0 * padding)).min(THUMB_MAX_SCALE),
        padding,
        background: false,
        dark: a.dark,
        clip_to_frame: false,
        max_side_px: f64::from(side) + 2.0,
    };
    let picture = render_png(s, id, &it.document, &opts)?;
    Ok(ThumbnailOut {
        changed: false,
        item: it.id.clone(),
        width: picture.width,
        height: picture.height,
        bytes: ExportBytes { base64: base64_of(&picture.bytes), mime: "image/png" },
    })
}

fn thumbnail_available(s: &Session) -> Result<(), Refusal> {
    let id = "library.thumbnail";
    needs_storage(s, id)?;
    if s.has_port(PortKind::Renderer) {
        Ok(())
    } else {
        Err(Refusal::port_unavailable(id, "the renderer", "renderer port missing", "Pictures of library items are drawn by the app."))
    }
}

// ---- library.import ----

/// How an imported library meets the existing one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// Add the items and skip duplicates (same content).
    #[default]
    Merge,
    /// Replace the whole library. Not undoable.
    Replace,
    /// Lay the items out on a grid in a new tab and leave the library alone (the old app's behaviour).
    OpenAsTab,
}

/// Arguments of `library.import`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportArgs {
    /// The `.excalidrawlib` file: `{path}`, `{stored}`, `{handle}` or `{base64, name?}`.
    pub source: Source,
    /// `merge` (default), `replace` or `open_as_tab`.
    #[serde(default)]
    pub mode: ImportMode,
}

/// Output of `library.import`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ImportOut {
    /// True when the library or the tabs changed.
    pub changed: bool,
    /// Items added to the library.
    pub added: usize,
    /// Items skipped because the library already has them (or the file has them twice).
    pub skipped: usize,
    /// The section the new items went to (named after the file), when there are any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Items in the library now.
    pub total: usize,
    /// The new tab, for `open_as_tab`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
}

fn not_a_library(id: &str, e: &IoError) -> Refusal {
    let why = match e {
        IoError::WrongFormat { hint, .. } => hint.clone(),
        other => other.to_string(),
    };
    Refusal::custom(
        "parse_failed",
        id,
        format!("The file is not an Excalidraw library: {}. Libraries are .excalidrawlib files.", files::cut(&why)),
    )
}

fn parse_library(s: &mut Session, id: &str, loaded: &Loaded) -> Result<excalidraw::Library, Refusal> {
    let text = std::str::from_utf8(&loaded.bytes)
        .map_err(|_| not_a_library(id, &IoError::parse(FormatName::ExcalidrawLibrary, "the data is not UTF-8 text")))?;
    let (lib, report) = excalidraw::import_library(text, &mut *s.ports.ids).map_err(|e| not_a_library(id, &e))?;
    for w in report.warnings {
        s.warn(w);
    }
    if report.omitted > 0 {
        s.warn(format!("{} more import notes were left out", report.omitted));
    }
    Ok(lib)
}

/// What [`merge_items`] did.
struct Merged {
    added: usize,
    skipped: usize,
    total: usize,
    /// The section the new items went to.
    section: Option<String>,
}

/// The section for a library imported from the file `file`: the one an earlier import of a file with that name
/// made, else a new one. The name is that of the file without its extension, `Library` when that is empty.
fn section_for(s: &mut Session, sections: &mut Vec<Section>, file: &str) -> Section {
    let name = normalize_name("library.import", &stem(file)).unwrap_or_else(|_| "Library".to_owned());
    if let Some(known) = sections.iter().find(|sec| sec.name == name) {
        return known.clone();
    }
    let id = store::fresh_section_id(s, sections);
    let section = Section { id, name, created_ms: s.now_ms() };
    sections.push(section.clone());
    section
}

/// Adds the items of a library file to the library, in the section of that file; items that exist (same content)
/// are skipped.
fn merge_items(s: &mut Session, id: &str, lib: excalidraw::Library, file: &str, replace: bool) -> Result<Merged, Refusal> {
    store::writable(s, id)?;
    let old = store::library(s);
    let mut now = if replace { Library { items: Vec::new(), sections: Vec::new() } } else { Library { items: old.items.clone(), sections: old.sections.clone() } };
    let mut touched: Vec<String> = Vec::new();
    let (mut added, mut skipped) = (0usize, 0usize);
    let created = s.now_ms();
    let mut section: Option<Section> = None;
    for LibraryItem { name, mut document } in lib.items {
        if !frag::normalize(&mut document) {
            skipped += 1;
            continue;
        }
        let hash = frag::content_hash(&document);
        if now.items.iter().any(|i| i.hash == hash) {
            skipped += 1;
            continue;
        }
        if now.items.len() >= MAX_LIBRARY_ITEMS {
            return Err(Refusal::limit_exceeded(id, now.items.len() + 1, "library items", MAX_LIBRARY_ITEMS));
        }
        let home = Home::Section(section.get_or_insert_with(|| section_for(s, &mut now.sections, file)).id.clone());
        let name = name.and_then(|n| normalize_name(id, &n).ok());
        let item = Arc::new(Item { id: store::fresh_id(s, &now.items), name, created_ms: created, hash, document, home });
        touched.push(item.id.clone());
        now.items.push(item);
        added += 1;
    }
    let removed: Vec<String> = if replace { old.items.iter().map(|i| i.id.clone()).collect() } else { Vec::new() };
    let total = now.items.len();
    if added > 0 || !removed.is_empty() {
        store::commit(s, now, &touched, &removed)?;
    }
    Ok(Merged { added, skipped, total, section: section.map(|sec| sec.id) })
}

/// `doc.drop` hands a library file here: merge mode. Returns the number of items added.
pub(crate) fn import_loaded(s: &mut Session, id: &str, loaded: &Loaded) -> Result<usize, Refusal> {
    needs_storage(s, id)?;
    let lib = parse_library(s, id, loaded)?;
    merge_items(s, id, lib, &loaded.name, false).map(|m| m.added)
}

fn stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = base.to_lowercase();
    let cut = [".excalidrawlib", ".json"].iter().find_map(|e| lower.ends_with(e).then(|| base.len() - e.len()));
    let s = cut.and_then(|n| base.get(..n)).unwrap_or(base).trim();
    if s.is_empty() || s == "the data" { "Library".to_owned() } else { s.to_owned() }
}

fn import(s: &mut Session, a: ImportArgs) -> Result<ImportOut, Refusal> {
    let id = s.cmd();
    let loaded = files::read_source(s, id, &a.source, "source")?;
    let lib = parse_library(s, id, &loaded)?;
    store::flush_notes(s);
    match a.mode {
        ImportMode::OpenAsTab => {
            let mut doc = excalidraw::library_to_grid_document(&lib, &mut *s.ports.ids);
            doc.name = stem(&loaded.name).chars().take(drafftink_scene::limits::MAX_NAME_CHARS).collect();
            open_in_new_tab(s, id, &doc, None, Provenance::Clean)?;
            let total = store::items(s).len();
            Ok(ImportOut { changed: true, added: 0, skipped: 0, section: None, total, tab: Some(s.tab().id.to_string()) })
        }
        ImportMode::Merge | ImportMode::Replace => {
            let replace = a.mode == ImportMode::Replace;
            let Merged { added, skipped, total, section } = merge_items(s, id, lib, &loaded.name, replace)?;
            Ok(ImportOut { changed: added > 0 || replace, added, skipped, section, total, tab: None })
        }
    }
}

// ---- library.export ----

/// Arguments of `library.export`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ExportArgs {
    /// Item ids or names. Default: all items.
    pub items: Option<Vec<String>>,
    /// `inline` (default) returns the bytes; a file is written through the files port.
    pub to: Dest,
}

/// Output of `library.export`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ExportOut {
    /// Always false.
    pub changed: bool,
    /// Items written.
    pub items: usize,
    /// Size in bytes.
    pub size: usize,
    /// The content, for `to: inline`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<ExportBytes>,
    /// Where it was written, for a file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_to: Option<FileRef>,
}

const LIB_MIME: &str = "application/vnd.excalidrawlib+json";

fn export(s: &mut Session, a: ExportArgs) -> Result<ExportOut, Refusal> {
    let id = s.cmd();
    let all = store::items(s);
    store::flush_notes(s);
    if all.is_empty() {
        return Err(Refusal::custom("nothing_to_do", id, "The library is empty, so there is nothing to export."));
    }
    let chosen: Vec<&Arc<Item>> = match &a.items {
        Some(list) => {
            let mut v: Vec<&Arc<Item>> = Vec::new();
            for w in list {
                if let Some(i) = all.get(find(id, &all, w)?)
                    && !v.iter().any(|x| x.id == i.id)
                {
                    v.push(i);
                }
            }
            v
        }
        None => all.iter().collect(),
    };
    if chosen.is_empty() {
        return Err(Refusal::custom("nothing_to_do", id, "No items were chosen, so there is nothing to export."));
    }
    if let Dest::File(f) = &a.to {
        files::require_writable(s, id, f)?;
    }
    let lib = excalidraw::Library {
        items: chosen.iter().map(|i| LibraryItem { name: i.name.clone(), document: i.document.clone() }).collect(),
    };
    let (text, report) = excalidraw::export_library_with_report(&lib)
        .map_err(|e| Refusal::io_failed(id, "write", "the library", &e.to_string()))?;
    for w in report.warnings {
        s.warn(w);
    }
    let bytes = text.into_bytes();
    let mut out = ExportOut { changed: false, items: chosen.len(), size: bytes.len(), bytes: None, saved_to: None };
    match a.to {
        Dest::Inline(_) => out.bytes = Some(ExportBytes { base64: base64_of(&bytes), mime: LIB_MIME }),
        Dest::File(f) => {
            files::queue_file(s, id, &f, bytes, LIB_MIME)?;
            out.saved_to = Some(f);
        }
    }
    Ok(out)
}

// ---- registration ----

/// A library command: storage and the library buffer, allowed in view-only mode, refused during a drag.
const LIB: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::PORTS,
    needs_selection: Selection::No,
    ports: PortSet::STORAGE,
    batch: Batch::Allowed,
    view_only: Gate::Allowed,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::None,
    ring: RingMode::Always,
    palette: true,
    agent: true,
    doc_write: crate::spec::DocWrite::NotDocument,
};

/// The library family.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "library.add",
        "library",
        "Add to library",
        "Stores a copy of the selected (or given) elements as a new library item.",
        &Typed::new(add),
        Facts { needs_selection: Selection::Required, ..LIB },
    )
    .host(Host::effect(HostEffect::CompletesLibraryAdd))
    .available(|s| needs_storage(s, "library.add"))
    .refusals(&["empty_name", "library_newer"])
    .example(r#"{"name":"Login box"}"#),
    CommandSpec::new(
        "library.insert",
        "library",
        "Insert from library",
        "Inserts a library item like a paste: new ids, centred on a point, selected.",
        &Typed::new(insert),
        Facts {
            undo: Undo::Undoable,
            mutates: Mutates::DOC.with(Mutates::OBSERVED),
            view_only: Gate::Refused,
            redraw: Redraw::Scene,
            doc_write: crate::spec::DocWrite::Edit,
            ..LIB
        },
    )
    .available(|s| needs_storage(s, "library.insert"))
    .refusals(&["no_such_item"])
    .example(r#"{"item":"Login box","at":[400,300]}"#),
    CommandSpec::new(
        "library.remove",
        "library",
        "Remove from library",
        "Removes library items by id or name, or all of them with all=true. Not undoable.",
        &Typed::new(remove),
        LIB,
    )
    .available(|s| needs_storage(s, "library.remove"))
    .refusals(&["no_such_item", "library_newer"])
    .example(r#"{"items":["Login box"]}"#),
    CommandSpec::new(
        "library.remove_section",
        "library",
        "Remove library section",
        "Removes a section of the library (an imported library, or your own items) with all its items. Not undoable.",
        &Typed::new(remove_section),
        LIB,
    )
    .available(|s| needs_storage(s, "library.remove_section"))
    .refusals(&["no_such_section", "library_newer"])
    .example(r#"{"section":"shapes"}"#),
    CommandSpec::new(
        "library.rename_section",
        "library",
        "Rename library section",
        "Gives an imported library's section a new name. Your own items keep the name My items.",
        &Typed::new(rename_section),
        LIB,
    )
    .host(Host::effect(HostEffect::CompletesRename))
    .available(|s| needs_storage(s, "library.rename_section"))
    .refusals(&["no_such_section", "empty_name", "library_newer"])
    .example(r#"{"section":"shapes","name":"Flowchart shapes"}"#),
    CommandSpec::new(
        "library.rename",
        "library",
        "Rename library item",
        "Gives a library item a new name.",
        &Typed::new(rename),
        LIB,
    )
    .host(Host::effect(HostEffect::CompletesRename))
    .available(|s| needs_storage(s, "library.rename"))
    .refusals(&["no_such_item", "empty_name", "library_newer"])
    .example(r#"{"item":"lib_e1","name":"Login box"}"#),
    CommandSpec::new(
        "library.list",
        "library",
        "List library",
        "Lists the library items with id, section, name, element count, bounds and creation time, and the sections.",
        &Typed::new(list),
        Facts { reads_only: true, mutates: Mutates::NONE, view_only: Gate::Allowed, while_gesture: Gate::Allowed, while_text_edit: TextGate::Allowed, ..LIB },
    )
    .available(|s| needs_storage(s, "library.list"))
    .refusals(&["no_such_section"]),
    CommandSpec::new(
        "library.thumbnail",
        "library",
        "Draw a library item",
        "Draws a library item as a small PNG with a transparent background, in the light or the dark palette.",
        &Typed::new(thumbnail),
        Facts {
            reads_only: true,
            mutates: Mutates::NONE,
            ports: PortSet::STORAGE.with(PortSet::RENDERER),
            view_only: Gate::Allowed,
            while_gesture: Gate::Allowed,
            while_text_edit: TextGate::Allowed,
            palette: false,
            ..LIB
        },
    )
    .available(thumbnail_available)
    .refusals(&["no_such_item"])
    .example(r#"{"item":"lib_e1","max_side":128,"dark":true}"#),
    CommandSpec::new(
        "library.import",
        "library",
        "Import library",
        "Reads an .excalidrawlib file (v1 or v2) and merges it into the library, replaces the library, or opens it as a tab.",
        &Typed::new(import),
        Facts { mutates: Mutates::PORTS.with(Mutates::TABS), ports: PortSet::FILES.with(PortSet::STORAGE), ..LIB },
    )
    .available(|s| needs_storage(s, "library.import"))
    .refusals(&["library_newer", "too_many_tabs"])
    .example(r#"{"source":{"path":"/home/me/shapes.excalidrawlib"}}"#),
    CommandSpec::new(
        "library.export",
        "library",
        "Export library",
        "Writes the library (or some items) as an .excalidrawlib v2 file, inline or to a file.",
        &Typed::new(export),
        Facts { ports: PortSet::FILES.with(PortSet::STORAGE), ..LIB },
    )
    .available(|s| needs_storage(s, "library.export"))
    .refusals(&["no_such_item"]),
];

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
