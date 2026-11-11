//! `doc.new`, `doc.open`, `doc.drop` and `doc.show_intro`: bring a document into the session.
//!
//! All four end in one of three placements: replace the active tab's document, open it in a new tab, or
//! merge it into the active document as new elements. The parsing is `drafftink-io` (`open`), the
//! merge rule is `drafftink_edit::library::insert_fragment`. Every check runs before the first change.

use drafftink_edit::library::{self, InsertReport, LibraryError, Placement};
use drafftink_geom::kurbo::Point;
use drafftink_io::native::{self, Format};
use drafftink_io::{IoError, Opened, png_meta};
use drafftink_scene::{Document, DocumentError, SceneRead, StoreError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::files::{self, InlineBytes, Loaded, Provenance, Source, base64_of};
use super::save;
use crate::args;
use crate::cmd::view::Pt;
use crate::cmd::tab::{MAX_TABS, OnUnsaved, RoomBinding, activate_new, blank_document, normalize_name, tab_from, too_many_tabs};
use crate::ports::FileRef;
use crate::refusal::{Refusal, echo};
use crate::session::Session;
use crate::spec::Redraw;
use crate::store::TabDoc;
use crate::state::{BASE_ZOOM, Observed};

/// The built-in welcome document, in the format of the old app (migrated when shown).
const INTRO_JSON: &str = include_str!("../../../assets/intro.json");
/// Name of the shown intro document.
const INTRO_NAME: &str = "Welcome";

/// Where an opened document goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Replace the document of the active tab.
    #[default]
    Replace,
    /// Open it in a new tab.
    NewTab,
    /// Add its elements to the active document, with new ids.
    Merge,
}

/// The file format `doc.open` expects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FormatWord {
    /// Decide from the content.
    #[default]
    Auto,
    /// Drafft.ink JSON (also the old app's JSON, gzip or Loro snapshot).
    Drafftink,
    /// An Excalidraw scene or clipboard.
    Excalidraw,
    /// A PNG (or SVG) with the scene embedded.
    Png,
}

/// What an open, new or intro command did.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct OpenOut {
    /// True when the session changed.
    pub changed: bool,
    /// Where the document went.
    pub mode: Mode,
    /// The name of the document now in the active tab.
    pub name: String,
    /// The active tab after the command.
    pub tab: String,
    /// What the bytes were.
    pub format: Detected,
    /// Elements in the opened document (for merge: elements added).
    pub elements: usize,
    /// Elements selected afterwards.
    pub selected: usize,
}

/// What the bytes of an opened document were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Detected {
    /// Drafft.ink JSON, old drafft-ink JSON, gzip or a Loro snapshot.
    Drafftink,
    /// An Excalidraw scene or clipboard.
    Excalidraw,
    /// A PNG with the scene embedded.
    Png,
    /// An SVG with the scene embedded.
    Svg,
    /// A new blank document (`doc.new`).
    Blank,
    /// The built-in intro document (`doc.show_intro`).
    Intro,
}

impl Detected {
    /// The kind of document a sniffed format is; `None` for formats that are not documents.
    fn of(f: Format) -> Option<Self> {
        match f {
            Format::Native | Format::LegacyJson | Format::LegacyClipboard | Format::LegacySnapshot | Format::Gzip => {
                Some(Self::Drafftink)
            }
            Format::ExcalidrawScene | Format::ExcalidrawClipboard => Some(Self::Excalidraw),
            Format::Png => Some(Self::Png),
            Format::Svg => Some(Self::Svg),
            Format::ExcalidrawLibrary | Format::Mermaid | Format::Unknown => None,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Drafftink => "drafftink",
            Self::Excalidraw => "excalidraw",
            Self::Png => "png",
            Self::Svg => "svg",
            Self::Blank => "blank",
            Self::Intro => "intro",
        }
    }

    fn matches(self, want: FormatWord) -> bool {
        match want {
            FormatWord::Auto => true,
            FormatWord::Drafftink => self == Self::Drafftink,
            FormatWord::Excalidraw => self == Self::Excalidraw,
            FormatWord::Png => matches!(self, Self::Png | Self::Svg),
        }
    }
}

impl FormatWord {
    fn word(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Drafftink => "drafftink",
            Self::Excalidraw => "excalidraw",
            Self::Png => "png",
        }
    }
}

// ---- refusals ----

const OPEN_HINT: &str = "Open a .json, .drafftink or .excalidraw file, or a PNG exported with the scene embedded.";

fn parse_failed(id: &str, msg: &str) -> Refusal {
    Refusal::custom(
        "parse_failed",
        id,
        format!("The file is not a drafft.ink or Excalidraw document: {}. {OPEN_HINT}", files::cut(msg)),
    )
}

/// Maps a reader error to the refusal the catalog names for it.
pub(super) fn io_refusal(id: &str, e: &IoError) -> Refusal {
    match e {
        IoError::Document(DocumentError::NewerVersion { found, max }) => Refusal::custom(
            "newer_version",
            id,
            format!(
                "This file was saved by a newer drafft.ink (format {found}), and this one reads up to format {max}. Update the app to open it."
            ),
        ),
        IoError::Limit { what, max } => Refusal::custom(
            "limit_exceeded",
            id,
            format!("The {what} is too large: the limit is {}. Split the file or remove elements.", echo(max)),
        ),
        other => parse_failed(id, &other.to_string()),
    }
}

fn is_library(id: &str, name: &str) -> Refusal {
    Refusal::custom(
        "is_library",
        id,
        format!("`{}` is an Excalidraw library, not a document. Add it to the library with `library.import`.", echo(name)),
    )
    .suggesting("library.import", None)
}

fn is_diagram(id: &str, name: &str) -> Refusal {
    Refusal::custom(
        "parse_failed",
        id,
        format!(
            "`{}` is Mermaid diagram text, not a document. Draw it with `diagram.import_mermaid`. {OPEN_HINT}",
            echo(name)
        ),
    )
    .suggesting("diagram.import_mermaid", None)
}

/// What a user can do instead of replacing the document of a tab that is in a room.
#[derive(Debug, Clone, Copy)]
pub(super) enum Alternative {
    /// `doc.new`: `in_new_tab=true`.
    Flag,
    /// `doc.open`, `doc.show_intro`: `mode=new_tab`.
    Mode,
    /// `doc.drop`: `documents=new_tab`.
    Documents,
}

impl Alternative {
    fn words(self) -> &'static str {
        match self {
            Self::Flag => "or pass in_new_tab=true to open the new document in a new tab",
            Self::Mode => "or pass mode=new_tab to open the file in a new tab",
            Self::Documents => "or pass documents=new_tab to open the files in new tabs",
        }
    }
}

fn in_room(id: &str, room: &str, instead: Alternative) -> Refusal {
    Refusal::custom(
        "in_room",
        id,
        format!(
            "This tab is connected to room {}, and a new document would replace the shared one. Leave the room with `collab.leave_room`, {}.",
            echo(room),
            instead.words()
        ),
    )
    .suggesting("collab.leave_room", None)
}

fn store_refusal(id: &str, e: &StoreError) -> Refusal {
    match e {
        StoreError::TooManyElements(n) => Refusal::custom(
            "limit_exceeded",
            id,
            format!("The document would hold more than {n} elements, which is the limit. Remove elements or open the file in a new tab."),
        ),
        other => Refusal::internal(id, other.to_string()),
    }
}

fn library_refusal(id: &str, e: LibraryError) -> Refusal {
    match e {
        LibraryError::Empty => Refusal::nothing_to_do(id, "add", "the file has no elements"),
        LibraryError::NotFinite => Refusal::bad_arg(id, "at", "must be finite numbers"),
        LibraryError::Store(s) => store_refusal(id, &s),
        other => Refusal::internal(id, other.to_string()),
    }
}

// ---- guards ----

/// The room the active tab has joined.
fn room(s: &Session) -> Option<String> {
    s.tab().ext.get::<RoomBinding>().and_then(|r| r.0.clone())
}

/// The checks before the active document is replaced: not in a room, and unsaved changes are refused,
/// dropped or saved (`on_unsaved`).
pub(super) fn guard_replace(s: &mut Session, id: &str, on_unsaved: OnUnsaved, instead: Alternative) -> Result<(), Refusal> {
    if let Some(r) = room(s) {
        return Err(in_room(id, &r, instead));
    }
    if !s.tab().dirty() {
        return Ok(());
    }
    match on_unsaved {
        OnUnsaved::Refuse => Err(Refusal::unsaved_changes(id)),
        OnUnsaved::Discard => Ok(()),
        OnUnsaved::Save => save::save_active_now(s, id, &save::SaveReq::default()).map(|_| ()),
    }
}

// ---- placements ----

/// Fits the camera of the active tab to its content, or resets it when the document is empty.
fn fit_view(s: &mut Session) {
    if !s.store().is_empty() && s.registry().find("view.zoom_to_fit").is_some() {
        if let Err(r) = s.run_nested("view.zoom_to_fit", json!({ "scope": "all" })) {
            s.warn(format!("the camera was not fitted: {}", r.message));
        }
        return;
    }
    let v = &mut s.tab_mut().view;
    v.scroll = Point::ZERO;
    v.zoom = BASE_ZOOM;
}

/// Replaces the document of the active tab. History, selection and the editor state are reset, the camera
/// fits the content, and the save target becomes `target`. A [`Provenance::Autosave`] document counts as
/// having unsaved work.
pub(super) fn replace_active(
    s: &mut Session,
    id: &str,
    doc: &Document,
    target: Option<FileRef>,
    provenance: Provenance,
) -> Result<(), Refusal> {
    if s.tab().doc.blocked_by_schema().is_some() {
        // A document from a newer version is read-only. Replacing it starts a fresh store, which is how it is
        // left (docs/crdt.md §8: "reopening the document clears it").
        s.tab_mut().doc = TabDoc::new();
    }
    let (tab, mark) = (s.tab().id, s.tab().doc.mark());
    s.tab_mut().doc.load(doc).map_err(|e| store_refusal(id, &e))?;
    // The executor runs the invariant pass after the handler. Run it now, so that what it repairs in a loaded
    // file (labels, arrows) is part of the state recorded as saved. The second run finds nothing to do.
    crate::invariants::run(s, tab, mark).map_err(|e| Refusal::internal(id, format!("an invariant broke: {e}")))?;
    s.clear_history();
    let t = s.tab_mut();
    t.observed = Observed::default();
    t.saved_revision = provenance.saved_revision(t.doc.revision());
    files::set_save_target(s, target);
    fit_view(s);
    s.mark_dirty(Redraw::Scene);
    Ok(())
}

/// Opens the document in a new tab after the active one.
pub(crate) fn open_in_new_tab(
    s: &mut Session,
    id: &str,
    doc: &Document,
    target: Option<FileRef>,
    provenance: Provenance,
) -> Result<(), Refusal> {
    if s.tabs.len() >= MAX_TABS {
        return Err(too_many_tabs(id));
    }
    let mut tab = tab_from(s, id, doc)?;
    tab.saved_revision = provenance.saved_revision(tab.doc.revision());
    tab.ext.set(files::SaveTarget(target));
    activate_new(s, tab);
    fit_view(s);
    s.mark_dirty(Redraw::Scene);
    Ok(())
}

/// Adds the elements of `doc` to the active document with new ids. `at` is the centre of the new block;
/// without it the elements keep their coordinates. Returns the report.
pub(crate) fn merge_into_active(
    s: &mut Session,
    id: &str,
    doc: &Document,
    at: Option<Pt>,
    select: bool,
) -> Result<InsertReport, Refusal> {
    if s.tab().doc.blocked_by_schema().is_some() {
        return Err(Refusal::newer_schema(id));
    }
    let place = match at {
        Some(p) => Placement::Centre(Point::new(args::world(id, "at[0]", p[0])?, args::world(id, "at[1]", p[1])?)),
        None => Placement::AsIs,
    };
    let i = s.active_index();
    let ids = &mut *s.ports.ids;
    let Some(tab) = s.tabs.get_mut(i) else { return Err(Refusal::internal(id, "the active tab disappeared")) };
    let (report, _) = tab
        .doc
        .edit::<InsertReport, LibraryError>(|tx| library::insert_fragment(tx, doc, place, ids))
        .map_err(|e| library_refusal(id, e))?;
    if select {
        tab.observed.selection = report.inserted.iter().cloned().collect();
        tab.observed.selected_groups = report.outer_groups.iter().cloned().collect();
    }
    s.mark_dirty(Redraw::Scene);
    Ok(report)
}

// ---- opening ----

/// How to open some bytes.
pub(super) struct OpenSpec<'a> {
    pub mode: Mode,
    pub format: FormatWord,
    /// What to do with unsaved changes when replacing. `None`: the guard already ran (a drop of several
    /// files runs it once).
    pub on_unsaved: Option<OnUnsaved>,
    pub at: Option<Pt>,
    pub select: bool,
    pub loaded: &'a Loaded,
}

/// Reads the bytes as a document and puts it where `spec.mode` says.
pub(super) fn open_document(s: &mut Session, id: &str, spec: &OpenSpec<'_>) -> Result<OpenOut, Refusal> {
    let opened = drafftink_io::open(&spec.loaded.bytes, &mut *s.ports.ids, s.services.io()).map_err(|e| io_refusal(id, &e))?;
    let (imported, source) = match opened {
        Opened::Document { imported, source } => (imported, source),
        Opened::Library { .. } => return Err(is_library(id, &spec.loaded.name)),
        Opened::Diagram(_) => return Err(is_diagram(id, &spec.loaded.name)),
    };
    let Some(format) = Detected::of(source) else {
        return Err(Refusal::internal(id, "the reader returned a document for bytes that are not a document"));
    };
    if !format.matches(spec.format) {
        return Err(parse_failed(
            id,
            &format!("it is {} data, but format={} was asked. Use format=auto", format.word(), spec.format.word()),
        ));
    }
    for w in &imported.report.warnings {
        s.warn(w.clone());
    }
    if imported.report.omitted > 0 {
        s.warn(format!("{} more import notes were left out", imported.report.omitted));
    }
    let doc = imported.document;
    let count = doc.elements.iter().filter(|e| !e.deleted).count();
    // Only a file in the drafft.ink format becomes the save target. Saving over an Excalidraw, legacy, PNG or
    // SVG file would replace it with drafft.ink JSON, so `doc.save` then asks where to write (save-as).
    let target = spec.loaded.file.clone().filter(|_| source == Format::Native);
    // The recent list still remembers the file that was opened.
    let opened_file = spec.loaded.file.clone();
    match spec.mode {
        Mode::Replace => {
            if let Some(on_unsaved) = spec.on_unsaved {
                guard_replace(s, id, on_unsaved, Alternative::Mode)?;
            }
            replace_active(s, id, &doc, target.clone(), spec.loaded.provenance)?;
        }
        Mode::NewTab => open_in_new_tab(s, id, &doc, target.clone(), spec.loaded.provenance)?,
        Mode::Merge => {
            let r = merge_into_active(s, id, &doc, spec.at, spec.select)?;
            return Ok(OpenOut {
                changed: true,
                mode: spec.mode,
                name: s.store().meta().name.clone(),
                tab: s.tab().id.to_string(),
                format,
                elements: r.inserted.len(),
                selected: if spec.select { r.inserted.len() } else { 0 },
            });
        }
    }
    if let Some(f) = &opened_file {
        files::remember(s, f)?;
    }
    Ok(OpenOut {
        changed: true,
        mode: spec.mode,
        name: s.store().meta().name.clone(),
        tab: s.tab().id.to_string(),
        format,
        elements: count,
        selected: 0,
    })
}

// ---- doc.open ----

/// Arguments of `doc.open`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenArgs {
    /// The file: `{path}`, `{stored}`, `{handle}`, or `{base64, name?, mime?}`. `{stored: "autosave"}` is the
    /// last autosave.
    pub source: Source,
    /// The expected format. `auto` decides from the content.
    #[serde(default)]
    pub format: FormatWord,
    /// `replace` (default) loads into the active tab, `new_tab` opens a tab, `merge` adds the elements.
    #[serde(default)]
    pub mode: Mode,
    /// What to do with unsaved changes when replacing: `refuse` (default), `discard` or `save`.
    #[serde(default)]
    pub on_unsaved: OnUnsaved,
    /// For merge: the world point to centre the new elements on. Default: their own coordinates.
    #[serde(default)]
    pub at: Option<Pt>,
    /// For merge: select the new elements. Default true.
    #[serde(default = "yes")]
    pub select: bool,
}

fn yes() -> bool {
    true
}

pub(super) fn open(s: &mut Session, a: OpenArgs) -> Result<OpenOut, Refusal> {
    let id = s.cmd();
    let loaded = files::read_source(s, id, &a.source, "source")?;
    let spec = OpenSpec {
        mode: a.mode,
        format: a.format,
        on_unsaved: Some(a.on_unsaved),
        at: a.at,
        select: a.select,
        loaded: &loaded,
    };
    open_document(s, id, &spec)
}

// ---- doc.new ----

/// Arguments of `doc.new`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct NewArgs {
    /// Name of the new document (1 to 120 characters). Default: "Untitled".
    pub name: Option<String>,
    /// What to do with unsaved changes when replacing: `refuse` (default), `discard` or `save`.
    pub on_unsaved: OnUnsaved,
    /// Open the new document in a new tab instead of replacing the active one.
    pub in_new_tab: bool,
}

pub(super) fn new(s: &mut Session, a: NewArgs) -> Result<OpenOut, Refusal> {
    let id = s.cmd();
    let name = match a.name.as_deref() {
        Some(n) => Some(normalize_name(id, n)?),
        None => None,
    };
    if a.in_new_tab && s.tabs.len() >= MAX_TABS {
        return Err(too_many_tabs(id));
    }
    let doc = blank_document(s, name);
    let mode = if a.in_new_tab { Mode::NewTab } else { Mode::Replace };
    if a.in_new_tab {
        open_in_new_tab(s, id, &doc, None, Provenance::Clean)?;
    } else {
        guard_replace(s, id, a.on_unsaved, Alternative::Flag)?;
        replace_active(s, id, &doc, None, Provenance::Clean)?;
    }
    Ok(OpenOut {
        changed: true,
        mode,
        name: doc.name,
        tab: s.tab().id.to_string(),
        format: Detected::Blank,
        elements: 0,
        selected: 0,
    })
}

// ---- doc.show_intro ----

/// Where the intro goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntroMode {
    /// Replace the document of the active tab.
    #[default]
    Replace,
    /// Open it in a new tab.
    NewTab,
}

/// Arguments of `doc.show_intro`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct IntroArgs {
    /// `replace` (default) or `new_tab`.
    pub mode: IntroMode,
    /// What to do with unsaved changes when replacing: `refuse` (default), `discard` or `save`.
    pub on_unsaved: OnUnsaved,
}

/// The built-in welcome document, migrated from the old app's format with the session's text measure.
pub fn intro_document(s: &mut Session, id: &str) -> Result<Document, Refusal> {
    let opened = drafftink_io::open(INTRO_JSON.as_bytes(), &mut *s.ports.ids, s.services.io()).map_err(|e| io_refusal(id, &e))?;
    match opened {
        Opened::Document { imported, .. } => {
            let mut doc = imported.document;
            doc.name = INTRO_NAME.to_owned();
            Ok(doc)
        }
        _ => Err(Refusal::internal(id, "the built-in intro is not a document")),
    }
}

pub(super) fn show_intro(s: &mut Session, a: IntroArgs) -> Result<OpenOut, Refusal> {
    let id = s.cmd();
    if a.mode == IntroMode::NewTab && s.tabs.len() >= MAX_TABS {
        return Err(too_many_tabs(id));
    }
    let doc = intro_document(s, id)?;
    let count = doc.elements.iter().filter(|e| !e.deleted).count();
    let mode = match a.mode {
        IntroMode::Replace => {
            guard_replace(s, id, a.on_unsaved, Alternative::Mode)?;
            replace_active(s, id, &doc, None, Provenance::Clean)?;
            Mode::Replace
        }
        IntroMode::NewTab => {
            open_in_new_tab(s, id, &doc, None, Provenance::Clean)?;
            Mode::NewTab
        }
    };
    Ok(OpenOut {
        changed: true,
        mode,
        name: doc.name,
        tab: s.tab().id.to_string(),
        format: Detected::Intro,
        elements: count,
        selected: 0,
    })
}

// ---- doc.drop ----

/// Most files one drop handles.
const DROP_MAX: usize = 50;
/// Most images one drop inserts (the limit of `image.insert`).
const DROP_IMAGES_MAX: usize = 20;

/// One dropped file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DropItem {
    /// The file name.
    pub name: String,
    /// The content: `{base64}` or `{path}`.
    pub bytes: Source,
    /// The media type, if known.
    #[serde(default)]
    pub mime: Option<String>,
}

/// What to do with a PNG that carries a scene.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScenePng {
    /// Insert it as an image (default).
    #[default]
    Insert,
    /// Restore the scene, as `doc.open` does.
    Open,
}

/// Where a dropped document goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DropDocs {
    /// A new tab (default).
    #[default]
    NewTab,
    /// Replace the active document.
    Replace,
    /// Merge into the active document.
    Merge,
}

/// Arguments of `doc.drop`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DropArgs {
    /// The dropped files (1 to 50).
    pub items: Vec<DropItem>,
    /// World point to put images and merged elements at. Default: the centre of the viewport.
    #[serde(default)]
    pub at: Option<Pt>,
    /// A PNG with an embedded scene: `insert` it as an image (default) or `open` the scene.
    #[serde(default)]
    pub scene_png: ScenePng,
    /// Documents: `new_tab` (default), `replace` or `merge`.
    #[serde(default)]
    pub documents: DropDocs,
    /// What to do with unsaved changes when a document replaces the active one.
    #[serde(default)]
    pub on_unsaved: OnUnsaved,
}

/// What a drop did.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DropOut {
    /// True when anything was opened, inserted or imported.
    pub changed: bool,
    /// Documents opened.
    pub documents: usize,
    /// Images inserted.
    pub images: usize,
    /// Library items added.
    pub library_items: usize,
    /// Files that were skipped (each is also a warning).
    pub skipped: usize,
}

enum Class {
    Image,
    Document,
    Library,
    Skip(String),
}

fn is_jpeg(b: &[u8]) -> bool {
    b.starts_with(&[0xff, 0xd8, 0xff])
}

fn is_webp(b: &[u8]) -> bool {
    b.len() >= 12 && b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP")
}

fn classify(bytes: &[u8], scene_png: ScenePng) -> Class {
    match native::sniff(bytes) {
        Format::Png => {
            let has_scene = png_meta::extract_scene(bytes).ok().flatten().is_some();
            if has_scene && scene_png == ScenePng::Open { Class::Document } else { Class::Image }
        }
        Format::ExcalidrawLibrary => Class::Library,
        Format::Native
        | Format::LegacyJson
        | Format::LegacySnapshot
        | Format::Gzip
        | Format::ExcalidrawScene
        | Format::ExcalidrawClipboard => Class::Document,
        Format::Svg => Class::Skip("SVG pictures are not supported; only SVG files exported from drafft.ink with a scene can be opened with `doc.open`".to_owned()),
        Format::LegacyClipboard | Format::Mermaid => Class::Skip("it is clipboard or diagram text, not a file".to_owned()),
        Format::Unknown if is_jpeg(bytes) || is_webp(bytes) => Class::Image,
        Format::Unknown => Class::Skip("it is not an image, document or library".to_owned()),
    }
}

pub(super) fn drop_files(s: &mut Session, a: DropArgs) -> Result<DropOut, Refusal> {
    let id = s.cmd();
    if a.items.len() > DROP_MAX {
        return Err(Refusal::limit_exceeded(id, a.items.len(), "files", DROP_MAX));
    }
    let n = a.items.len();
    // Read and sort everything first; nothing has changed yet.
    let mut docs: Vec<Loaded> = Vec::new();
    let mut libs: Vec<Loaded> = Vec::new();
    let mut images: Vec<(Loaded, Option<String>)> = Vec::new();
    let mut skipped = 0usize;
    for (i, item) in a.items.iter().enumerate() {
        let mut loaded = match files::read_source(s, id, &item.bytes, &format!("items[{i}].bytes")) {
            Ok(l) => l,
            Err(r) if r.code == "bad_args" => return Err(r),
            Err(r) => {
                s.warn(format!("`{}` was skipped: {}", echo(&item.name), r.message));
                skipped += 1;
                continue;
            }
        };
        loaded.name = item.name.clone();
        match classify(&loaded.bytes, a.scene_png) {
            Class::Image => images.push((loaded, item.mime.clone())),
            Class::Document => docs.push(loaded),
            Class::Library => libs.push(loaded),
            Class::Skip(why) => {
                s.warn(format!("`{}` was skipped: {why}", echo(&item.name)));
                skipped += 1;
            }
        }
    }
    if docs.is_empty() && libs.is_empty() && images.is_empty() {
        return Err(Refusal::nothing_to_do(
            id,
            "open",
            &format!(
                "none of the {n} dropped files is an image, document or library. Supported: PNG, JPEG, WebP, .json, .drafftink, .excalidraw, .excalidrawlib"
            ),
        ));
    }
    let mut refused_images: Option<String> = None;
    if images.len() > DROP_IMAGES_MAX {
        return Err(Refusal::limit_exceeded(id, images.len(), "images", DROP_IMAGES_MAX));
    }
    if !images.is_empty() && s.registry().find("image.insert").is_none() {
        let why = "`image.insert` is not available in this session";
        s.warn(format!("{} image(s) were not inserted: {why}", images.len()));
        skipped += images.len();
        refused_images = Some(why.to_owned());
        images.clear();
    }
    let replace = !docs.is_empty() && a.documents == DropDocs::Replace;
    if replace {
        guard_replace(s, id, a.on_unsaved, Alternative::Documents)?;
    }
    let mut out = DropOut { changed: false, documents: 0, images: 0, library_items: 0, skipped };
    let mode = match a.documents {
        DropDocs::NewTab => Mode::NewTab,
        DropDocs::Replace => Mode::Replace,
        DropDocs::Merge => Mode::Merge,
    };
    for loaded in &docs {
        let spec = OpenSpec {
            mode,
            format: FormatWord::Auto,
            on_unsaved: None,
            at: a.at,
            select: true,
            loaded,
        };
        match open_document(s, id, &spec) {
            Ok(_) => {
                out.documents += 1;
                out.changed = true;
            }
            Err(r) => {
                s.warn(format!("`{}` was skipped: {}", echo(&loaded.name), r.message));
                out.skipped += 1;
            }
        }
    }
    for loaded in &libs {
        match crate::cmd::library::import_loaded(s, id, loaded) {
            Ok(added) => {
                out.library_items += added;
                out.changed |= added > 0;
            }
            Err(r) => {
                s.warn(format!("`{}` was skipped: {}", echo(&loaded.name), r.message));
                out.skipped += 1;
            }
        }
    }
    if !images.is_empty() {
        let list: Vec<_> = images
            .iter()
            .map(|(l, mime)| {
                let b = InlineBytes { base64: base64_of(&l.bytes), name: Some(l.name.clone()), mime: mime.clone() };
                json!({ "base64": b.base64, "name": b.name, "mime": b.mime })
            })
            .collect();
        let at = a.at.unwrap_or_else(|| {
            let v = s.view();
            [v.scroll.x, v.scroll.y]
        });
        match s.run_nested("image.insert", json!({ "images": list, "at": at, "offset": 20 })) {
            Ok(_) => {
                out.images = images.len();
                out.changed = true;
            }
            Err(r) => {
                s.warn(format!("{} image(s) were not inserted: {}", images.len(), r.message));
                out.skipped += images.len();
                refused_images = Some(r.message);
            }
        }
    }
    if !out.changed {
        let why = refused_images.map(|m| format!(": {}", m.trim_end_matches('.'))).unwrap_or_default();
        return Err(Refusal::nothing_to_do(id, "open", &format!("none of the {n} dropped files could be opened{why}")));
    }
    Ok(out)
}
