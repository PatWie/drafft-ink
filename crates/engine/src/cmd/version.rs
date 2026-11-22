//! `version`: named versions of the document (docs/commands.md §2.24, design in `docs/design/versions.md`).
//!
//! A version is a name, a time and a compressed copy of the scene, kept inside the document (`drafftink_scene::
//! NamedVersion`). It is saved in the file, the autosave and the CRDT, so it survives a restart and syncs in a room.
//! The rules are in the domain crates: how a version is made and read in `scene`, how it is restored in
//! `edit::version`. These commands check what the user asked for and delegate.
//!
//! * `version.save`, `version.rename` and `version.delete` write document data that is not the scene. They are not
//!   undo steps: Ctrl+Z is for the scene, and it must never delete a name the user gave (`DocWrite::Metadata`).
//! * `version.restore` is an ordinary edit: one undo step, and in a room it syncs like any other edit. It keeps every
//!   named version, so a restore can itself be taken back, or be undone by restoring the version made just before it.
//! * `version.open` puts a version in a tab of its own, as an independent copy.

use drafftink_edit::version::{self as restore_rule, RestoreError};
use drafftink_scene::limits::{
    MAX_NAME_CHARS, MAX_VERSION_AUTHOR_CHARS, MAX_VERSION_NAME_CHARS, MAX_VERSIONS, MAX_VERSIONS_TOTAL_BYTES,
};
use drafftink_scene::{
    Document, NamedVersion, SceneRead, SceneTx, StoreError, VersionError, VersionId, VersionRejection,
    VersionScene,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::tab::{MAX_TABS, activate_new, tab_from, too_many_tabs};
use crate::cmd::element_common::prune_selection;
use crate::refusal::{Refusal, echo};
use crate::session::Session;
use crate::spec::{
    Batch, CommandSpec, DocWrite, Facts, Gate, Mutates, NoArgs, PortSet, Redraw, RingMode, Selection, TextGate, Typed,
    Undo,
};

/// What every command that writes document data outside the scene shares.
const METADATA: Facts = Facts {
    undo: Undo::NoUndo,
    reads_only: false,
    mutates: Mutates::DOC,
    needs_selection: Selection::No,
    ports: PortSet::NONE,
    batch: Batch::Allowed,
    view_only: Gate::Refused,
    while_gesture: Gate::Refused,
    while_text_edit: TextGate::Commit,
    redraw: Redraw::Overlay,
    ring: RingMode::Always,
    palette: true,
    agent: true,
    doc_write: DocWrite::Metadata,
    secret_output: false,
};

/// A restore is an ordinary edit of the scene.
const RESTORES: Facts = Facts {
    palette: false,
    undo: Undo::Undoable,
    mutates: Mutates::DOC.with(Mutates::OBSERVED),
    redraw: Redraw::Scene,
    doc_write: DocWrite::Edit,
    ..METADATA
};

/// Opening a version adds a tab: tabs only, no undo, allowed in view-only mode, and nothing may follow it in a
/// batch, as with `tab.new`.
const OPENS: Facts = Facts {
    palette: false,
    mutates: Mutates::TABS,
    batch: Batch::Last,
    view_only: Gate::Allowed,
    redraw: Redraw::Scene,
    doc_write: DocWrite::NotDocument,
    ..METADATA
};

/// A version as `version.list` shows it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct VersionInfo {
    /// The id to pass to the other `version` commands.
    pub id: String,
    /// The name.
    pub name: String,
    /// When it was saved, in milliseconds since the Unix epoch.
    pub created_ms: u64,
    /// Who saved it, when they had set a name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Live elements in the scene it holds.
    pub elements: u32,
    /// Bytes the stored scene takes.
    pub bytes: usize,
}

impl VersionInfo {
    fn of(v: &NamedVersion) -> Self {
        Self {
            id: v.id.to_string(),
            name: v.name.clone(),
            created_ms: v.created_ms,
            author: v.author.clone(),
            elements: v.elements,
            bytes: v.stored_bytes(),
        }
    }
}

// ---- refusals ----

fn name_refusal(id: &str, e: &VersionError) -> Refusal {
    match e {
        VersionError::BadName => Refusal::custom(
            "empty_name",
            id,
            format!(
                "The version needs a name of 1 to {MAX_VERSION_NAME_CHARS} characters without line breaks, for example \"Before review\"."
            ),
        ),
        other => Refusal::internal(id, other.to_string()),
    }
}

fn no_such_version(id: &str, which: &str) -> Refusal {
    Refusal::custom(
        "no_such_version",
        id,
        format!("There is no version `{}` in this document. List the versions with `version.list`.", echo(which)),
    )
    .suggesting("version.list", None)
}

fn unreadable(id: &str, v: &NamedVersion, e: &VersionError) -> Refusal {
    Refusal::custom(
        "version_unreadable",
        id,
        format!(
            "The version \"{}\" cannot be read: {e}. Nothing was changed. Delete it with `version.delete`, or use another version.",
            echo(&v.name)
        ),
    )
    .suggesting("version.delete", Some(serde_json::json!({ "id": v.id.as_str() })))
}

fn parse_id(id: &str, raw: &str) -> Result<VersionId, Refusal> {
    VersionId::new(raw.trim()).map_err(|_| no_such_version(id, raw))
}

/// The version named by `raw`, read from the working store of the active tab.
fn find(s: &Session, id: &str, raw: &str) -> Result<NamedVersion, Refusal> {
    let key = parse_id(id, raw)?;
    s.store().version(&key).cloned().ok_or_else(|| no_such_version(id, raw))
}

fn store_refusal(id: &str, e: &StoreError) -> Refusal {
    match e {
        StoreError::BadVersion(VersionRejection::Invalid(v)) => name_refusal(id, v),
        other => Refusal::internal(id, other.to_string()),
    }
}

fn available_loaded(s: &Session) -> Result<(), Refusal> {
    super::collab::require_loaded(s, s.cmd())
}

/// The name other people see, when the user has set one: the author of a version.
fn author(s: &Session) -> Option<String> {
    s.settings
        .get("collab.user_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty() && !n.chars().any(char::is_control))
        .map(|n| n.chars().take(MAX_VERSION_AUTHOR_CHARS).collect())
}

// ---- version.save ----

/// Arguments of `version.save`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveArgs {
    /// The name, 1 to 100 characters, trimmed, no line breaks. It need not be unique: versions are addressed by id.
    pub name: String,
}

/// Result of `version.save`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SaveOut {
    /// True.
    pub changed: bool,
    /// The new version.
    pub version: VersionInfo,
    /// Versions the document holds now.
    pub count: usize,
    /// Most versions `version.save` keeps.
    pub limit: usize,
}

fn too_many(id: &str) -> Refusal {
    Refusal::custom(
        "too_many_versions",
        id,
        format!(
            "This document already has {MAX_VERSIONS} named versions, which is the limit. Delete one you no longer need with `version.delete`, then save again."
        ),
    )
    .suggesting("version.list", None)
}

fn save(s: &mut Session, a: SaveArgs) -> Result<SaveOut, Refusal> {
    let id = s.cmd();
    let name = drafftink_scene::normalize_version_name(&a.name).map_err(|e| name_refusal(id, &e))?;
    let (count, bytes) = s.store().versions().fold((0usize, 0usize), |(n, b), v| (n + 1, b + v.stored_bytes()));
    if count >= MAX_VERSIONS {
        return Err(too_many(id));
    }
    let (new_id, now, who) = (s.ports.ids.version_id(), s.now_ms(), author(s));
    let v = NamedVersion::capture(new_id, &name, now, who.as_deref(), s.store()).map_err(|e| match e {
        VersionError::StateTooLarge { len, max } => Refusal::custom(
            "version_too_large",
            id,
            format!(
                "The scene is too large to keep as a named version: it takes {} KiB once compressed, and a version may take {} KiB. Export the document to a file instead, or remove some of it first.",
                len / 1024,
                max / 1024
            ),
        ),
        other => name_refusal(id, &other),
    })?;
    if bytes + v.stored_bytes() > MAX_VERSIONS_TOTAL_BYTES {
        return Err(Refusal::custom(
            "versions_too_large",
            id,
            format!(
                "The named versions of this document already take {} MiB, and together they may take {} MiB. Delete one you no longer need with `version.delete`, then save again.",
                bytes / (1024 * 1024),
                MAX_VERSIONS_TOTAL_BYTES / (1024 * 1024)
            ),
        )
        .suggesting("version.list", None));
    }
    let info = VersionInfo::of(&v);
    s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.put_version(v)).map_err(|e| store_refusal(id, &e))?;
    Ok(SaveOut { changed: true, version: info, count: count + 1, limit: MAX_VERSIONS })
}

// ---- version.list ----

/// Result of `version.list`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ListOut {
    /// Always false.
    pub changed: bool,
    /// The versions, newest first.
    pub versions: Vec<VersionInfo>,
    /// How many there are.
    pub count: usize,
    /// Most versions `version.save` keeps.
    pub limit: usize,
}

fn list(s: &mut Session, _: NoArgs) -> Result<ListOut, Refusal> {
    let mut versions: Vec<VersionInfo> = s.store().versions().map(VersionInfo::of).collect();
    versions.reverse();
    Ok(ListOut { changed: false, count: versions.len(), versions, limit: MAX_VERSIONS })
}

// ---- version.open ----

/// Where `version.open` shows the version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenMode {
    /// An independent copy in a new tab, named "<document> — <version>". Editing it changes nothing in the
    /// document it came from, and it keeps none of that document's versions.
    #[default]
    NewTab,
}

/// Arguments of `version.open`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenArgs {
    /// The version, by the id that `version.list` shows.
    pub id: String,
    /// `new_tab` (the only mode, and the default).
    #[serde(default)]
    pub mode: OpenMode,
}

/// Result of `version.open`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct OpenOut {
    /// True.
    pub changed: bool,
    /// The new tab.
    pub tab: String,
    /// Its position, counted from 0.
    pub index: usize,
    /// Its name.
    pub name: String,
    /// The version it shows.
    pub version: String,
}

/// "<document> — <version>", cut to the longest document name.
fn tab_name(document: &str, version: &str) -> String {
    const SEPARATOR: &str = " — ";
    let version: String = version.chars().take(MAX_NAME_CHARS / 2).collect();
    let room = MAX_NAME_CHARS.saturating_sub(version.chars().count() + SEPARATOR.chars().count());
    let document: String = document.chars().take(room).collect();
    format!("{document}{SEPARATOR}{version}")
}

/// The files and elements of `scene` as a document: the blobs of the open document it needs, and nothing else.
fn as_document(s: &Session, name: String, v: &NamedVersion, scene: VersionScene) -> Document {
    let files = v.files.iter().filter_map(|f| s.store().file(f).map(|e| (f.clone(), e.clone()))).collect();
    Document { name, background: scene.background, elements: scene.elements, files, ..Document::default() }
}

fn open(s: &mut Session, a: OpenArgs) -> Result<OpenOut, Refusal> {
    let id = s.cmd();
    if s.tabs.len() >= MAX_TABS {
        return Err(too_many_tabs(id));
    }
    let v = find(s, id, &a.id)?;
    let scene = v.open().map_err(|e| unreadable(id, &v, &e))?;
    let name = tab_name(&s.store().meta().name, &v.name);
    let doc = as_document(s, name.clone(), &v, scene);
    if let Some(issue) = doc.validate().errors().next() {
        return Err(unreadable(id, &v, &VersionError::Corrupt(issue.to_string())));
    }
    let tab = tab_from(s, id, &doc)?;
    let tab_id = tab.id;
    activate_new(s, tab);
    Ok(OpenOut { changed: true, tab: tab_id.to_string(), index: s.active_index(), name, version: v.id.to_string() })
}

// ---- version.restore ----

/// Arguments of `version.restore`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreArgs {
    /// The version, by the id that `version.list` shows.
    pub id: String,
}

/// Result of `version.restore`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RestoreOut {
    /// False when the scene already was the scene of the version.
    pub changed: bool,
    /// The version.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Elements that were gone or deleted and are back.
    pub brought_back: usize,
    /// Elements that changed to what the version has.
    pub reset: usize,
    /// Elements the version does not have, now deleted.
    pub deleted: usize,
    /// Pictures the version shows that the document no longer holds.
    pub missing_pictures: usize,
}

fn restore(s: &mut Session, a: RestoreArgs) -> Result<RestoreOut, Refusal> {
    let id = s.cmd();
    let v = find(s, id, &a.id)?;
    let scene = v.open().map_err(|e| unreadable(id, &v, &e))?;
    let (r, _) = s.tab_mut().doc.edit::<_, RestoreError>(|tx| restore_rule::restore(tx, &scene)).map_err(|e| match e {
        RestoreError::Invalid(why) => unreadable(id, &v, &VersionError::Corrupt(why)),
        RestoreError::Store(e) => Refusal::internal(id, format!("restoring \"{}\" failed: {e}", echo(&v.name))),
    })?;
    prune_selection(s);
    if !r.missing_files.is_empty() {
        s.warn(format!(
            "{} picture(s) of \"{}\" are no longer in the document and show as missing.",
            r.missing_files.len(),
            echo(&v.name)
        ));
    }
    Ok(RestoreOut {
        changed: !r.is_empty(),
        id: v.id.to_string(),
        name: v.name,
        brought_back: r.brought_back,
        reset: r.changed,
        deleted: r.deleted,
        missing_pictures: r.missing_files.len(),
    })
}

// ---- version.rename ----

/// Arguments of `version.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameArgs {
    /// The version, by the id that `version.list` shows.
    pub id: String,
    /// The new name, 1 to 100 characters, trimmed, no line breaks.
    pub name: String,
}

/// Result of `version.rename` and `version.delete`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ChangeOut {
    /// False when the version already had this name.
    pub changed: bool,
    /// The version.
    pub id: String,
    /// Its name after the command (for `version.delete`, the name it had).
    pub name: String,
    /// Versions the document holds now.
    pub count: usize,
}

fn rename(s: &mut Session, a: RenameArgs) -> Result<ChangeOut, Refusal> {
    let id = s.cmd();
    let name = drafftink_scene::normalize_version_name(&a.name).map_err(|e| name_refusal(id, &e))?;
    let v = find(s, id, &a.id)?;
    if v.name != name {
        s.tab_mut().doc.edit::<(), StoreError>(|tx| tx.rename_version(&v.id, &name)).map_err(|e| store_refusal(id, &e))?;
    }
    Ok(ChangeOut { changed: v.name != name, id: v.id.to_string(), name, count: s.store().versions().count() })
}

// ---- version.delete ----

/// Arguments of `version.delete`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteArgs {
    /// The version, by the id that `version.list` shows.
    pub id: String,
}

fn delete(s: &mut Session, a: DeleteArgs) -> Result<ChangeOut, Refusal> {
    let id = s.cmd();
    let v = find(s, id, &a.id)?;
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            tx.remove_version(&v.id);
            Ok(())
        })
        .map_err(|e| store_refusal(id, &e))?;
    Ok(ChangeOut { changed: true, id: v.id.to_string(), name: v.name, count: s.store().versions().count() })
}

/// The version commands.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new(
        "version.save",
        "version",
        "Name this version",
        "Names the current state of the document: keeps a copy of the scene with the name, the time and, when set, your name. Not an undo step.",
        &Typed::new(save),
        METADATA,
    )
    .host(crate::spec::Host::effect(crate::spec::HostEffect::CompletesRename))
    .available(available_loaded)
    .refusals(&["empty_name", "too_many_versions", "version_too_large", "versions_too_large", "room_loading"])
    .example(r#"{"name":"Before review"}"#),
    CommandSpec::new(
        "version.list",
        "version",
        "List versions",
        "Lists the named versions of the document, newest first, with id, name, time, author and element count.",
        &Typed::new(list),
        Facts { palette: false, ..Facts::READ_ONLY },
    ),
    CommandSpec::new(
        "version.open",
        "version",
        "Open version as copy",
        "Opens a named version as an independent copy in a new tab, named \"<document> — <version>\".",
        &Typed::new(open),
        OPENS,
    )
    .available(available_loaded)
    .refusals(&["no_such_version", "version_unreadable", "too_many_tabs", "room_loading"])
    .example(r#"{"id":"v1","mode":"new_tab"}"#),
    CommandSpec::new(
        "version.restore",
        "version",
        "Restore version",
        "Makes the scene of a named version the current scene, as one undo step. Every named version is kept.",
        &Typed::new(restore),
        RESTORES,
    )
    .available(available_loaded)
    .refusals(&["no_such_version", "version_unreadable", "room_loading"])
    .example(r#"{"id":"v1"}"#),
    CommandSpec::new(
        "version.rename",
        "version",
        "Rename version",
        "Changes the name of a named version. Not an undo step.",
        &Typed::new(rename),
        Facts { palette: false, ..METADATA },
    )
    .refusals(&["no_such_version", "empty_name"])
    .host(crate::spec::Host::effect(crate::spec::HostEffect::CompletesRename))
    .example(r#"{"id":"v1","name":"Final draft"}"#),
    CommandSpec::new(
        "version.delete",
        "version",
        "Delete version",
        "Deletes a named version for good. The scene is not changed. Not an undo step.",
        &Typed::new(delete),
        Facts { palette: false, ..METADATA },
    )
    .refusals(&["no_such_version"])
    .example(r#"{"id":"v1"}"#),
];

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
