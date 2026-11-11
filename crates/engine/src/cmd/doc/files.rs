//! Where document bytes come from and go to: the `source` argument, `FileRef`s over the `Files` and `Storage`
//! ports, the save target of a tab, and the recent list. Used by `doc` and `library`.

use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cmd::settings;
use crate::cmd::tab::NEVER_SAVED;
use crate::cmd::tab::workspace::{INDEX_KEY, TAB_KEY_PREFIX};
use crate::ports::{FileRef, Ns, PortKind, PortWrite};
use crate::refusal::{Refusal, echo};
use crate::session::Session;

/// The name under which `{stored: "autosave"}` reads the last autosave (the active tab of the workspace record).
pub const AUTOSAVE_NAME: &str = "autosave";
/// Most files in the recent list.
pub const RECENT_MAX: usize = 10;

pub use crate::cmd::bytes::InlineBytes;

/// Where the data of a command comes from: inline bytes, or a file or stored name that the session reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Source {
    /// `{base64, name?, mime?}`.
    Inline(InlineBytes),
    /// `{path}`, `{stored}`, `{download}` or `{handle}`. `{stored: "autosave"}` is the last autosave.
    File(FileRef),
}

/// Bytes read from a [`Source`].
pub struct Loaded {
    /// The content.
    pub bytes: Vec<u8>,
    /// A short name for messages: the file name, or `the data`.
    pub name: String,
    /// The file it came from, when it can be written back to (a path, a stored name, a handle).
    pub file: Option<FileRef>,
    /// Whether the content counts as saved.
    pub provenance: Provenance,
}

/// Whether bytes that were read count as saved work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// A file or inline data: opening it leaves the document clean.
    Clean,
    /// The last autosave: its content is work that was never saved.
    Autosave,
}

impl Provenance {
    /// The value for `Tab::saved_revision` of a document just loaded at `revision`.
    pub fn saved_revision(self, revision: u64) -> u64 {
        match self {
            Self::Clean => revision,
            Self::Autosave => NEVER_SAVED,
        }
    }
}

/// The last part of a path, or the stored name.
pub fn display_name(f: &FileRef) -> String {
    match f {
        FileRef::Path { path } => path.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path).to_owned(),
        FileRef::Stored { stored } => stored.clone(),
        FileRef::Download { download } => download.clone(),
        FileRef::Handle { handle } => handle.clone(),
    }
}

/// Decodes inline bytes (padding optional, a `data:` prefix accepted).
pub fn decode_inline(id: &str, key: &str, b: &InlineBytes) -> Result<Vec<u8>, Refusal> {
    b.decode().map_err(|_| Refusal::bad_arg(id, key, "is not valid base64"))
}

/// Standard base64 of some bytes.
pub fn base64_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn no_files_to_read(id: &str) -> Refusal {
    Refusal::port_unavailable(id, "file access", "files port missing", "Pass the data inline as `{base64, name}` instead.")
}

fn no_files_to_write(id: &str) -> Refusal {
    Refusal::port_unavailable(
        id,
        "file access",
        "files port missing",
        "To get the bytes without writing a file, use `doc.export` with to=\"inline\".",
    )
}

fn no_storage(id: &str) -> Refusal {
    Refusal::port_unavailable(id, "persistent storage", "storage port missing", "Use a path or a download name instead.")
}

/// Reads a [`Source`]. A missing port, an unreadable file or bad base64 is a refusal.
pub fn read_source(s: &Session, id: &str, src: &Source, key: &str) -> Result<Loaded, Refusal> {
    match src {
        Source::Inline(b) => Ok(Loaded {
            bytes: decode_inline(id, &format!("{key}.base64"), b)?,
            name: b.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "the data".to_owned()),
            file: None,
            provenance: Provenance::Clean,
        }),
        Source::File(f) => read_file(s, id, f),
    }
}

/// Reads a file through the port that serves it.
pub fn read_file(s: &Session, id: &str, f: &FileRef) -> Result<Loaded, Refusal> {
    let name = display_name(f);
    let fail = |e: crate::ports::PortError| Refusal::io_failed(id, "read", &name, &e.to_string());
    match f {
        FileRef::Stored { stored } if stored == AUTOSAVE_NAME => {
            let st = s.ports.storage.as_ref().ok_or_else(|| no_storage(id))?;
            let bytes = last_autosave(st.as_ref()).map_err(fail)?.ok_or_else(|| {
                Refusal::io_failed(id, "read", "the last autosave", "nothing has been autosaved yet")
            })?;
            Ok(Loaded { bytes, name: "the last autosave".to_owned(), file: None, provenance: Provenance::Autosave })
        }
        FileRef::Stored { stored } => {
            let st = s.ports.storage.as_ref().ok_or_else(|| no_storage(id))?;
            let bytes = st
                .get(Ns::Documents, stored)
                .map_err(fail)?
                .ok_or_else(|| Refusal::io_failed(id, "read", &name, "there is no stored document with this name"))?;
            Ok(Loaded { bytes, name, file: Some(f.clone()), provenance: Provenance::Clean })
        }
        _ => {
            let files = s.ports.files.as_ref().ok_or_else(|| no_files_to_read(id))?;
            let bytes = files.read(f).map_err(fail)?;
            let file = if matches!(f, FileRef::Download { .. }) { None } else { Some(f.clone()) };
            Ok(Loaded { bytes, name, file, provenance: Provenance::Clean })
        }
    }
}

/// The document of the active tab in the stored workspace record (see `cmd::tab::workspace`).
fn last_autosave(st: &dyn crate::ports::Storage) -> Result<Option<Vec<u8>>, crate::ports::PortError> {
    let Some(index) = st.get(Ns::Autosave, INDEX_KEY)? else { return Ok(None) };
    let Ok(index) = serde_json::from_slice::<serde_json::Value>(&index) else { return Ok(None) };
    let active = index.get("active").and_then(serde_json::Value::as_u64);
    let key = active.map(|n| format!("{TAB_KEY_PREFIX}{n}"));
    match key {
        Some(k) => st.get(Ns::Autosave, &k),
        None => Ok(None),
    }
}

/// When the last autosave was written, and its size, for `doc.list`.
pub fn last_autosave_info(s: &Session) -> Option<(String, u64, u64)> {
    let st = s.ports.storage.as_ref()?;
    let index: serde_json::Value = serde_json::from_slice(&st.get(Ns::Autosave, INDEX_KEY).ok()??).ok()?;
    let active = index.get("active")?.as_u64()?;
    let tabs = index.get("tabs")?.as_array()?;
    let entry = tabs.iter().find(|t| t.get("id").and_then(serde_json::Value::as_u64) == Some(active))?;
    let name = entry.get("name").and_then(serde_json::Value::as_str)?.to_owned();
    let key = format!("{TAB_KEY_PREFIX}{active}");
    let info = st.list(Ns::Autosave).ok()?.into_iter().find(|i| i.name == key)?;
    Some((name, info.modified_ms, info.size))
}

/// True when the target already holds something.
pub fn exists(s: &Session, f: &FileRef) -> bool {
    match f {
        FileRef::Stored { stored } => s
            .ports
            .storage
            .as_ref()
            .is_some_and(|st| st.get(Ns::Documents, stored).ok().flatten().is_some()),
        FileRef::Download { .. } => false,
        _ => s.ports.files.as_ref().is_some_and(|fl| fl.exists(f)),
    }
}

/// Checks that the port which serves `f` is there.
pub fn require_writable(s: &Session, id: &str, f: &FileRef) -> Result<(), Refusal> {
    match f {
        FileRef::Stored { .. } if !s.has_port(PortKind::Storage) => Err(no_storage(id)),
        FileRef::Stored { .. } => Ok(()),
        _ if !s.has_port(PortKind::Files) => Err(no_files_to_write(id)),
        _ => Ok(()),
    }
}

/// Queues the write of `bytes` to `f`. It happens after the command succeeded.
pub fn queue_file(s: &mut Session, id: &str, f: &FileRef, bytes: Vec<u8>, mime: &str) -> Result<(), Refusal> {
    require_writable(s, id, f)?;
    match f {
        FileRef::Stored { stored } => {
            s.queue_write(PortWrite::StoragePut { ns: Ns::Documents, key: stored.clone(), bytes });
        }
        _ => s.queue_write(PortWrite::File { file: f.clone(), bytes, mime: mime.to_owned() }),
    }
    Ok(())
}

// ---- save target ----

/// Where `doc.save` writes by default; per tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveTarget(pub Option<FileRef>);

/// Session setting of the web shell: with no save target, `doc.save` writes to `{stored: <document name>}`.
#[derive(Debug, Clone, Copy, Default)]
struct StoredDefault(bool);

/// The save target of the active tab.
pub fn save_target(s: &Session) -> Option<FileRef> {
    s.tab().ext.get::<SaveTarget>().and_then(|t| t.0.clone())
}

/// Sets the save target of the active tab.
pub fn set_save_target(s: &mut Session, f: Option<FileRef>) {
    s.tab_mut().ext.set(SaveTarget(f));
}

/// Makes `Mod+S` work without a dialog: with this on, a document without a save target is saved under its
/// name in the stored documents (the web shell turns it on at start). Session configuration, not a command.
pub fn use_stored_default_target(s: &mut Session, on: bool) {
    s.ext.set(StoredDefault(on));
}

/// True when the shell asked for the stored default target.
pub fn stored_default(s: &Session) -> bool {
    s.ext.get::<StoredDefault>().is_some_and(|d| d.0)
}

// ---- recent documents ----

/// The recent list: `doc.recent`, newest first.
pub fn recent(s: &Session) -> Vec<FileRef> {
    s.settings
        .get("doc.recent")
        .and_then(|v| serde_json::from_value::<Vec<FileRef>>(v.clone()).ok())
        .unwrap_or_default()
}

/// Puts a file at the top of the recent list (a download and the autosave are not remembered). The list is the
/// setting `doc.recent`, which only commands of this family write.
pub fn remember(s: &mut Session, f: &FileRef) -> Result<(), Refusal> {
    match f {
        FileRef::Download { .. } => return Ok(()),
        FileRef::Stored { stored } if stored == AUTOSAVE_NAME => return Ok(()),
        _ => {}
    }
    let mut list = recent(s);
    list.retain(|x| x != f);
    list.insert(0, f.clone());
    list.truncate(RECENT_MAX);
    settings::set_internal(s, "doc.recent", &json!(list))?;
    Ok(())
}

/// A parser message cut to 300 characters, so that hostile input cannot bloat a refusal.
pub fn cut(s: &str) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(300).collect();
    if it.next().is_some() { format!("{head}…") } else { head }
}

/// A short phrase for a file, for messages.
pub fn describe(f: &FileRef) -> String {
    match f {
        FileRef::Path { path } => echo(path),
        FileRef::Stored { stored } => format!("the stored document `{}`", echo(stored)),
        FileRef::Download { download } => format!("the download `{}`", echo(download)),
        FileRef::Handle { handle } => format!("the file {}", echo(handle)),
    }
}
