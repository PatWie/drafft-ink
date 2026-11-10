//! `doc.list`: the recent, stored and autosaved documents. (`doc.inspect` and `doc.outline` are in `doc_read`.)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::files::{self, AUTOSAVE_NAME};
use crate::ports::{FileRef, Ns, PortKind};
use crate::refusal::Refusal;
use crate::session::Session;

// ---- doc.list ----

/// Which documents `doc.list` lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListFrom {
    /// The last 10 documents saved or opened (default).
    #[default]
    Recent,
    /// The documents in the stored document namespace (the web document store).
    Stored,
    /// The last autosave.
    Autosave,
}

/// Arguments of `doc.list`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ListArgs {
    /// `recent` (default), `stored` or `autosave`.
    pub from: ListFrom,
}

/// One listed document.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DocInfo {
    /// A short name.
    pub name: String,
    /// What to pass to `doc.open` as `source`.
    #[serde(rename = "ref")]
    pub file: FileRef,
    /// When it was last written, if known.
    pub modified_ms: Option<u64>,
    /// Its size in bytes, if known.
    pub size: Option<u64>,
}

/// Output of `doc.list`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ListOut {
    /// Always false.
    pub changed: bool,
    /// The documents, newest first for `recent`.
    pub documents: Vec<DocInfo>,
}

pub(super) fn list(s: &mut Session, a: ListArgs) -> Result<ListOut, Refusal> {
    let id = s.cmd();
    let need_storage = || {
        Refusal::port_unavailable(id, "persistent storage", "storage port missing", "Use `from: recent` instead.")
    };
    let documents = match a.from {
        ListFrom::Recent => {
            let stored = s.ports.storage.as_ref().and_then(|st| st.list(Ns::Documents).ok()).unwrap_or_default();
            files::recent(s)
                .into_iter()
                .map(|f| {
                    let info = match &f {
                        FileRef::Stored { stored: n } => stored.iter().find(|i| i.name == *n),
                        _ => None,
                    };
                    DocInfo {
                        name: files::display_name(&f),
                        modified_ms: info.map(|i| i.modified_ms),
                        size: info.map(|i| i.size),
                        file: f,
                    }
                })
                .collect()
        }
        ListFrom::Stored => {
            let st = s.ports.storage.as_ref().ok_or_else(need_storage)?;
            st.list(Ns::Documents)
                .map_err(|e| Refusal::io_failed(id, "read", "the stored documents", &e.to_string()))?
                .into_iter()
                .map(|i| DocInfo {
                    name: i.name.clone(),
                    file: FileRef::Stored { stored: i.name },
                    modified_ms: Some(i.modified_ms),
                    size: Some(i.size),
                })
                .collect()
        }
        ListFrom::Autosave => {
            if !s.has_port(PortKind::Storage) {
                return Err(need_storage());
            }
            files::last_autosave_info(s)
                .map(|(name, modified_ms, size)| DocInfo {
                    name,
                    file: FileRef::Stored { stored: AUTOSAVE_NAME.to_owned() },
                    modified_ms: Some(modified_ms),
                    size: Some(size),
                })
                .into_iter()
                .collect()
        }
    };
    Ok(ListOut { changed: false, documents })
}
