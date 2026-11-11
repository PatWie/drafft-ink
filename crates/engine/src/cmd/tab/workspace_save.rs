//! The write of the workspace record, a slice at a time.
//!
//! A write of a big document is more work than a frame has: at 100,000 elements, 63 ms to copy the document, 20 ms to
//! check it and 115 ms to write 45 MB of JSON, then a native `fsync`. [`SaveJob`] takes the document without copying it
//! ([`crate::store::TabDoc::share_document`]), then writes it in pieces of bounded work, one piece per call of
//! `Session::workspace_tick`, and hands the finished text to the storage port with `put_later`. The text is the one
//! `Document::to_json` makes for the document at the moment the job began, byte for byte; a change made meanwhile
//! is a new write afterwards, as it always was.
//!
//! Work is counted in bytes of JSON: a piece is about [`TICK_BYTES`] of text, and the other steps cost what they cost
//! per element in the same unit ([`SHARE_COST`], [`CHECK_COST`]). A small document is written in one call.

use std::collections::VecDeque;

use drafftink_scene::{DocumentCheck, DocumentError, JsonJob, SharedDocument};
use serde_json::{Value, json};

use super::workspace::{INDEX_KEY, SCHEMA, TAB_KEY_PREFIX, WorkspaceSaved};
use crate::ports::Ns;
use crate::refusal::Refusal;
use crate::session::Session;
use crate::state::TabId;

const ID: &str = "autosave";

/// Work of one call, in bytes of JSON: about 5 ms in a release build (2.6 ns a byte).
pub(super) const TICK_BYTES: usize = 2 << 20;
/// What taking one element costs, in bytes of JSON (150 ns: the walk of the store's tree and a pointer count).
const SHARE_COST: usize = 60;
/// What checking one element costs, in bytes of JSON (200 ns; the last step of a check does the lists that need every
/// element and is not counted).
const CHECK_COST: usize = 80;

/// Which tab a document is, and what it was when it was taken.
#[derive(Debug)]
struct Meta {
    id: TabId,
    key: String,
    revision: u64,
    name: String,
}

/// A tab whose document is to be written.
#[derive(Debug)]
struct Pending {
    meta: Meta,
    doc: SharedDocument,
}

/// The step a document is at.
#[derive(Debug)]
enum Running {
    /// Looking for rules it breaks, so that only a document that reads back is written.
    Checking(Meta, SharedDocument, DocumentCheck),
    /// Written as JSON.
    Writing(Meta, JsonJob),
}

/// A document that is written.
#[derive(Debug)]
struct Written {
    meta: Meta,
    bytes: Vec<u8>,
}

/// What a finished write put on its way to the storage port.
pub(super) struct Finished {
    pub(super) saved: WorkspaceSaved,
    /// Tab, revision and name of each document that was written.
    pub(super) written: Vec<(TabId, u64, String)>,
}

/// One write of the workspace under way.
#[derive(Debug, Default)]
pub(super) struct SaveJob {
    /// The index entries of every tab, as they were when the job began.
    entries: Vec<Value>,
    tabs: Vec<TabId>,
    active: u32,
    /// The signature of the session when the job began.
    pub(super) sig: u64,
    queue: VecDeque<Pending>,
    running: Option<Running>,
    written: Vec<Written>,
}

fn document_error(tab: TabId, e: &DocumentError) -> Refusal {
    Refusal::internal(ID, format!("the document of tab {tab} cannot be written: {e}"))
}

impl SaveJob {
    /// Begins a write: the index entries, and a shared copy of every tab that changed since `written` says it was
    /// written. Returns the job and what it cost.
    pub(super) fn begin(
        s: &Session,
        written: &std::collections::BTreeMap<TabId, (u64, String)>,
        sig: u64,
    ) -> (Self, usize) {
        let mut job = SaveJob { sig, active: s.tab().id.0, ..SaveJob::default() };
        let mut cost = 0;
        for t in &s.tabs {
            let key = format!("{TAB_KEY_PREFIX}{}", t.id.0);
            let (revision, name) = (t.doc.revision(), super::name_of(t));
            if written.get(&t.id) != Some(&(revision, name.clone())) {
                let doc = t.doc.share_document();
                cost += doc.elements.len().saturating_mul(SHARE_COST);
                job.queue.push_back(Pending { meta: Meta { id: t.id, key: key.clone(), revision, name: name.clone() }, doc });
            }
            job.tabs.push(t.id);
            job.entries.push(json!({
                "id": t.id.0,
                "key": key,
                "name": name,
                "dirty": t.dirty(),
                "revision": revision,
                "view": { "scroll": [t.view.scroll.x, t.view.scroll.y], "zoom": t.view.zoom },
                "selection": t.observed.selection.iter().map(drafftink_scene::ElementId::as_str).collect::<Vec<_>>(),
            }));
        }
        (job, cost)
    }

    /// Does work until `budget` bytes of it are done, or the documents are all written. True when they are.
    pub(super) fn work(&mut self, mut budget: usize) -> Result<bool, Refusal> {
        loop {
            if budget == 0 {
                return Ok(self.queue.is_empty() && self.running.is_none());
            }
            match self.running.take() {
                None => {
                    let Some(Pending { meta, doc }) = self.queue.pop_front() else { return Ok(true) };
                    let check = doc.checker();
                    self.running = Some(Running::Checking(meta, doc, check));
                }
                Some(Running::Checking(meta, doc, mut check)) => {
                    let elements = (budget / CHECK_COST).max(1);
                    let done = check.step(&doc, elements);
                    budget = budget.saturating_sub(elements.min(doc.elements.len()).saturating_mul(CHECK_COST));
                    if done {
                        check.finish().map_err(|e| document_error(meta.id, &e))?;
                        let job = JsonJob::new(doc).map_err(|e| document_error(meta.id, &e))?;
                        self.running = Some(Running::Writing(meta, job));
                    } else {
                        self.running = Some(Running::Checking(meta, doc, check));
                    }
                }
                Some(Running::Writing(meta, mut job)) => {
                    let before = job.len();
                    let elements = job.step(budget).map_err(|e| document_error(meta.id, &e))?;
                    drafftink_profile::count_named("autosave_elements", u64::try_from(elements).unwrap_or(u64::MAX));
                    budget = budget.saturating_sub((job.len() - before).max(1));
                    if job.is_done() {
                        let bytes = job.finish().map_err(|e| document_error(meta.id, &e))?;
                        self.written.push(Written { meta, bytes });
                    } else {
                        self.running = Some(Running::Writing(meta, job));
                    }
                }
            }
        }
    }

    /// Hands the finished documents and then the index to the storage port. `cleanup` also lists the namespace and
    /// removes the documents of closed tabs, which waits for the port's queue.
    pub(super) fn finish(self, s: &mut Session, cleanup: bool) -> Result<Finished, Refusal> {
        let now_ms = s.now_ms();
        let index = json!({
            "format": "drafftink-workspace",
            "schema": SCHEMA,
            "saved_ms": now_ms,
            "active": self.active,
            "tabs": self.entries,
        });
        let index_bytes = serde_json::to_vec(&index).map_err(|e| Refusal::internal(ID, format!("the index is not JSON: {e}")))?;
        let Some(storage) = s.ports.storage.as_mut() else {
            return Err(Refusal::internal(ID, "the storage port disappeared"));
        };
        let mut done = Vec::with_capacity(self.written.len());
        let documents_written = self.written.len();
        for Written { meta: Meta { id, key, revision, name }, bytes } in self.written {
            storage
                .put_later(Ns::Autosave, &key, bytes)
                .map_err(|e| Refusal::io_failed(ID, "write", &format!("the workspace document `{key}`"), &e.to_string()))?;
            done.push((id, revision, name));
        }
        let index_len = index_bytes.len();
        storage
            .put_later(Ns::Autosave, INDEX_KEY, index_bytes)
            .map_err(|e| Refusal::io_failed(ID, "write", "the workspace index", &e.to_string()))?;
        if cleanup {
            // Documents of closed tabs. A failure here only leaves garbage, which the next save removes.
            let current: Vec<String> = self.tabs.iter().map(|t| format!("{TAB_KEY_PREFIX}{}", t.0)).collect();
            if let Ok(listed) = storage.list(Ns::Autosave) {
                for info in listed {
                    if info.name.starts_with(TAB_KEY_PREFIX) && !current.contains(&info.name) {
                        let _ = storage.delete(Ns::Autosave, &info.name);
                    }
                }
            }
        }
        Ok(Finished {
            saved: WorkspaceSaved { tabs: self.tabs.len(), documents_written, index_bytes: index_len },
            written: done,
        })
    }

    /// The tabs the record names.
    pub(super) fn tabs(&self) -> &[TabId] {
        &self.tabs
    }
}
