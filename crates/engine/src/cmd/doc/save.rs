//! `doc.save`, `doc.autosave`, `doc.rename`, `doc.set_background` and `doc.clear`.

use drafftink_edit::lifecycle::{self, ChildPolicy};
use drafftink_edit::style::ColorArg;
use drafftink_io::{excalidraw, native};
use drafftink_scene::{ElementId, SceneRead, SceneTx, StoreError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::files;
use crate::cmd::tab::normalize_name;
use crate::ports::{FileRef, PortKind};
use crate::refusal::Refusal;
use crate::session::Session;
use crate::state::Observed;

/// The file format `doc.save` writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SaveFormat {
    /// Drafft.ink JSON, versioned, images embedded.
    #[default]
    Drafftink,
    /// An Excalidraw scene; fields it cannot hold are dropped and listed in `warnings`.
    Excalidraw,
}

/// How to save.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveReq {
    /// Where to; `None` uses the save target of the tab.
    pub to: Option<FileRef>,
    /// The format.
    pub format: SaveFormat,
    /// Make `to` the save target and a recent file.
    pub remember: bool,
    /// Replace a file that exists and is not this document's file.
    pub overwrite: bool,
}

impl Default for SaveReq {
    fn default() -> Self {
        Self { to: None, format: SaveFormat::Drafftink, remember: true, overwrite: false }
    }
}

/// What a save did.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SaveOut {
    /// True: the document is no longer dirty.
    pub changed: bool,
    /// Where the document was written.
    pub saved_to: FileRef,
    /// The format written.
    pub format: SaveFormat,
    /// Size of the file in bytes.
    pub bytes: usize,
}

fn no_save_target(id: &str) -> Refusal {
    Refusal::custom(
        "no_save_target",
        id,
        "This document has no save location yet. Pass `to` (a path, a stored name or a download name).",
    )
}

/// Writes the active document and marks it saved. The write happens after the command succeeded.
pub(super) fn save_active(s: &mut Session, id: &str, req: &SaveReq) -> Result<SaveOut, Refusal> {
    let current = files::save_target(s);
    let target = match (&req.to, &current) {
        (Some(t), _) => t.clone(),
        (None, Some(t)) => t.clone(),
        (None, None) if files::stored_default(s) => FileRef::Stored { stored: s.store().meta().name.clone() },
        (None, None) => return Err(no_save_target(id)),
    };
    if req.to.is_some() && current.as_ref() != Some(&target) && !req.overwrite && files::exists(s, &target) {
        return Err(Refusal::custom(
            "would_overwrite",
            id,
            format!(
                "`{}` already exists and is not this document's file. Pass overwrite=true to replace it.",
                files::describe(&target)
            ),
        ));
    }
    files::require_writable(s, id, &target)?;
    let doc = s.tab().doc.snapshot();
    let (bytes, mime) = match req.format {
        SaveFormat::Drafftink => {
            let text = if matches!(target, FileRef::Stored { .. }) { native::save(&doc) } else { native::save_pretty(&doc) };
            (text.map_err(|e| Refusal::internal(id, format!("the document cannot be written: {e}")))?, "application/json")
        }
        SaveFormat::Excalidraw => {
            let (text, report) = excalidraw::export_scene_with_report(&doc)
                .map_err(|e| Refusal::internal(id, format!("the document cannot be written: {e}")))?;
            for w in report.warnings {
                s.warn(w);
            }
            (text, "application/vnd.excalidraw+json")
        }
    };
    let size = bytes.len();
    files::queue_file(s, id, &target, bytes.into_bytes(), mime)?;
    if req.remember {
        files::remember(s, &target)?;
        if !matches!(target, FileRef::Download { .. }) {
            files::set_save_target(s, Some(target.clone()));
        }
    }
    let t = s.tab_mut();
    t.saved_revision = t.doc.revision();
    Ok(SaveOut { changed: true, saved_to: target, format: req.format, bytes: size })
}

// ---- doc.save ----

/// Arguments of `doc.save`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SaveArgs {
    /// Where to write: `{path}`, `{stored}`, `{download}` or `{handle}`. Default: the save target of the document.
    pub to: Option<FileRef>,
    /// `drafftink` (default) or `excalidraw`.
    pub format: SaveFormat,
    /// Make `to` the save target and a recent file. Default true.
    pub remember: bool,
    /// Replace a file that exists and is not this document's file. Default false.
    pub overwrite: bool,
}

impl Default for SaveArgs {
    fn default() -> Self {
        Self { to: None, format: SaveFormat::Drafftink, remember: true, overwrite: false }
    }
}

pub(super) fn save(s: &mut Session, a: SaveArgs) -> Result<SaveOut, Refusal> {
    let id = s.cmd();
    save_active(s, id, &SaveReq { to: a.to, format: a.format, remember: a.remember, overwrite: a.overwrite })
}

// ---- doc.autosave ----

/// What an autosave did.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AutosaveOut {
    /// False: the command wrote nothing the document or the session needed.
    pub changed: bool,
    /// True when a document was written because it had changed since the last autosave.
    pub saved: bool,
    /// Tabs in the workspace record.
    pub tabs: usize,
}

/// The state check of `doc.autosave`.
pub(super) fn needs_storage(s: &Session) -> Result<(), Refusal> {
    if s.has_port(PortKind::Storage) {
        Ok(())
    } else {
        Err(Refusal::custom(
            "port_unavailable",
            "doc.autosave",
            "`doc.autosave` needs persistent storage, which this session does not provide (storage port missing).",
        ))
    }
}

pub(super) fn autosave(s: &mut Session, _: crate::spec::NoArgs) -> Result<AutosaveOut, Refusal> {
    let id = s.cmd();
    // Decision §9.6: the autosave is the workspace record (all tabs), written by `Session::save_workspace`.
    let r = s.save_workspace().map_err(|e| e.raised_by(id))?;
    Ok(AutosaveOut { changed: r.documents_written > 0, saved: r.documents_written > 0, tabs: r.tabs })
}

// ---- doc.rename ----

/// Arguments of `doc.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameArgs {
    /// The new name, 1 to 120 characters (trimmed).
    pub name: String,
}

/// Output of the commands that edit the document's own properties.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PropOut {
    /// False when the document already had this value.
    pub changed: bool,
    /// The name of the document after the command.
    pub name: String,
    /// The background colour after the command, `#rrggbb` or `#rrggbbaa`.
    pub background: String,
}

fn prop_out(s: &Session, changed: bool) -> PropOut {
    let m = s.store().meta();
    PropOut { changed, name: m.name.clone(), background: m.background.to_hex() }
}

pub(super) fn rename(s: &mut Session, a: RenameArgs) -> Result<PropOut, Refusal> {
    let id = s.cmd();
    let name = normalize_name(id, &a.name)?;
    if s.store().meta().name == name {
        return Ok(prop_out(s, false));
    }
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| tx.set_name(&name))
        .map_err(|e| Refusal::bad_arg(id, "name", e.to_string()))?;
    Ok(prop_out(s, true))
}

// ---- doc.set_background ----

/// Arguments of `doc.set_background`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackgroundArgs {
    /// `#rrggbb`, `#rrggbbaa`, `transparent`, or a palette name such as `gray-50`.
    pub color: ColorArg,
}

pub(super) fn set_background(s: &mut Session, a: BackgroundArgs) -> Result<PropOut, Refusal> {
    let id = s.cmd();
    if s.store().meta().background == a.color.0 {
        return Ok(prop_out(s, false));
    }
    s.tab_mut()
        .doc
        .edit::<(), StoreError>(|tx| {
            tx.set_background(a.color.0);
            Ok(())
        })
        .map_err(|e| Refusal::internal(id, e.to_string()))?;
    Ok(prop_out(s, true))
}

// ---- doc.clear ----

/// The state check of `doc.clear`.
pub(super) fn not_empty(s: &Session) -> Result<(), Refusal> {
    if s.store().is_empty() {
        Err(Refusal::custom("empty_document", "doc.clear", "The document is already empty, so there is nothing to clear."))
    } else {
        Ok(())
    }
}

/// What `doc.clear` removed.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ClearOut {
    /// True.
    pub changed: bool,
    /// Elements removed (bound text and frame children included).
    pub removed: usize,
}

pub(super) fn clear(s: &mut Session, _: crate::spec::NoArgs) -> Result<ClearOut, Refusal> {
    let id = s.cmd();
    not_empty(s).map_err(|e| e.raised_by(id))?;
    let ids: Vec<ElementId> = s.store().iter().map(|e| e.id.clone()).collect();
    let (report, _) = s
        .tab_mut()
        .doc
        .edit::<_, lifecycle::LifecycleError>(|tx| {
            // Locked elements go too: unlock them in the same step, then delete.
            lifecycle::lock(tx, &ids, Some(false))?;
            lifecycle::delete(tx, &ids, ChildPolicy::Delete)
        })
        .map_err(|e| Refusal::internal(id, e.to_string()))?;
    s.tab_mut().observed = Observed::default();
    Ok(ClearOut { changed: true, removed: report.deleted.len() })
}
