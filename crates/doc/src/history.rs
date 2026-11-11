//! History size and trimming.

use loro::ExportMode;

use crate::error::DocError;
use crate::store::{Core, LoroStore, check_size, decode_vv, import_fresh};

/// How big the history is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryStats {
    /// Operations in the log.
    pub ops: usize,
    /// Size in bytes of a full snapshot (state plus history).
    pub snapshot_bytes: usize,
    /// Size in bytes of a state-only snapshot (no history).
    pub state_only_bytes: usize,
    /// True when the history was already trimmed (the document started from a shallow snapshot).
    pub shallow: bool,
}

/// What to keep when trimming.
#[derive(Debug, Clone, Default)]
pub struct TrimOptions {
    /// Keep the history of this many recent local commits. 0 keeps none: the state is all that remains.
    pub keep_commits: usize,
    /// The version vector the server last acknowledged (as returned by `version_vector()` on the server side
    /// after it applied this peer's updates). Trimming refuses when this replica holds operations beyond it,
    /// because trimmed operations can no longer be sent. `None` for a document that is not synced.
    pub synced_to: Option<Vec<u8>>,
}

/// The effect of a trim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrimReport {
    /// Full snapshot size before.
    pub bytes_before: usize,
    /// Full snapshot size after.
    pub bytes_after: usize,
}

impl LoroStore {
    /// Sizes of the history. Exports twice, so call it when idle (on save, on a timer), not per frame.
    pub fn history_stats(&self) -> Result<HistoryStats, DocError> {
        let snap = self.export_snapshot()?;
        let state = self.export_state_only()?;
        Ok(HistoryStats {
            ops: self.core.guard(|| self.core.doc.len_ops())?,
            snapshot_bytes: snap.len(),
            state_only_bytes: state.len(),
            shallow: self.core.guard(|| self.core.doc.is_shallow())?,
        })
    }

    /// Replaces the Loro document with a trimmed copy that keeps the history of the last `keep_commits` local
    /// commits. The visible scene does not change. **The undo and redo stacks are cleared**: Loro cannot trim a
    /// document in place, so the store continues on a fresh document built from a shallow snapshot. Peers whose
    /// version is older than the kept history can no longer receive updates from this replica and must take a
    /// snapshot. Do it when the undo stack is empty or at a moment the user expects a reset (after save, after
    /// join).
    pub fn trim_history(&mut self, opts: &TrimOptions) -> Result<TrimReport, DocError> {
        if let Some(acked) = &opts.synced_to {
            let acked = decode_vv(acked)?;
            if !acked.includes_vv(&self.core.guard(|| self.core.doc.oplog_vv())?) {
                return Err(DocError::UnsyncedChanges);
            }
        }
        let before = self.export_snapshot()?.len();
        let frontiers = self.keep_frontiers(opts.keep_commits);
        let bytes = self
            .core
            .guard(|| self.core.doc.export(ExportMode::shallow_snapshot(&frontiers)))?
            .map_err(DocError::backend)?;
        check_size(bytes.len(), self.core.config.max_import_bytes)?;
        let fresh = import_fresh(Core::make_doc(&self.core.config)?, &bytes)?;
        let after = fresh.export(ExportMode::Snapshot).map_err(DocError::backend)?.len();
        self.core.reset_doc(fresh);
        Ok(TrimReport { bytes_before: before, bytes_after: after })
    }

    /// True when a full snapshot is bigger than `max_bytes` while the state alone would be under half of it,
    /// that is, when trimming would pay off.
    pub fn should_trim(&self, max_bytes: usize) -> Result<bool, DocError> {
        let s = self.history_stats()?;
        Ok(s.snapshot_bytes > max_bytes && s.state_only_bytes.saturating_mul(2) < s.snapshot_bytes)
    }
}
