//! The recent-command ring (docs/commands.md §1.9): a bounded log of top-level calls for `engine.recent`,
//! debugging and agents that check their own work. It is session state, not document state.

use std::collections::VecDeque;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::refusal::Refusal;

/// Entries kept.
pub const DEFAULT_CAPACITY: usize = 256;
/// Longest argument text kept per entry, in bytes.
pub const ARGS_MAX_BYTES: usize = 512;

/// Who made the call. Recorded in the ring; never changes the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A button, slider or other widget.
    Ui,
    /// A keyboard shortcut.
    #[serde(alias = "keyboard")]
    Shortcut,
    /// A menu item.
    Menu,
    /// A context menu item.
    ContextMenu,
    /// The command palette.
    Palette,
    /// A script or the console (`window.drafftink`). The default.
    #[default]
    Script,
    /// An agent through the bridge or MCP.
    #[serde(alias = "mcp")]
    Agent,
    /// Applied for a remote peer. Not recorded in the ring unless it fails.
    Remote,
    /// A test.
    Test,
}

impl Origin {
    /// The name used in JSON.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Shortcut => "shortcut",
            Self::Menu => "menu",
            Self::ContextMenu => "context_menu",
            Self::Palette => "palette",
            Self::Script => "script",
            Self::Agent => "agent",
            Self::Remote => "remote",
            Self::Test => "test",
        }
    }
}

/// What a call came to.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Outcome {
    /// It ran.
    Ok {
        /// Always true.
        ok: bool,
        /// Whether it changed any state.
        changed: bool,
        /// The undo label when it pushed a step.
        #[serde(skip_serializing_if = "Option::is_none")]
        undo_label: Option<String>,
    },
    /// It refused.
    Refused {
        /// Always false.
        ok: bool,
        /// The refusal code.
        code: String,
        /// The refusal text.
        message: String,
    },
}

impl Outcome {
    /// A success.
    pub fn ok(changed: bool, undo_label: Option<String>) -> Self {
        Self::Ok { ok: true, changed, undo_label }
    }

    /// A refusal.
    pub fn refused(r: &Refusal) -> Self {
        Self::Refused { ok: false, code: r.code.to_owned(), message: r.message.clone() }
    }

    /// True for a success.
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    /// The refusal code, if it refused.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Refused { code, .. } => Some(code),
            Self::Ok { .. } => None,
        }
    }
}

/// One recorded call.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RingEntry {
    /// Counts up from 1 over the life of the session.
    pub seq: u64,
    /// Wall time in milliseconds since the Unix epoch.
    pub t_ms: u64,
    /// The command id (for a batch, `batch`).
    pub id: String,
    /// The arguments as JSON text, cut to 512 bytes. Image bytes are replaced by their length.
    pub args: String,
    /// Who called.
    pub source: Origin,
    /// Success or refusal.
    pub outcome: Outcome,
    /// How long it took.
    pub duration_us: u64,
    /// For a batch: the number of steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<usize>,
    /// For a gesture: the number of frames.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<usize>,
    /// False for read-only commands.
    pub mutating: bool,
}

/// The ring.
#[derive(Debug, Clone)]
pub struct Ring {
    entries: VecDeque<RingEntry>,
    capacity: usize,
    next_seq: u64,
}

impl Default for Ring {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

/// What the executor hands the ring.
pub struct Record<'a> {
    /// Wall time.
    pub t_ms: u64,
    /// Command id.
    pub id: &'a str,
    /// Arguments, if any.
    pub args: Option<&'a Value>,
    /// Who called.
    pub source: Origin,
    /// The result.
    pub outcome: Outcome,
    /// Duration in microseconds.
    pub duration_us: u64,
    /// Batch steps.
    pub steps: Option<usize>,
    /// Gesture frames.
    pub frames: Option<usize>,
    /// True unless the command is read-only.
    pub mutating: bool,
}

impl Ring {
    /// An empty ring that keeps at most `capacity` entries.
    pub fn new(capacity: usize) -> Self {
        Self { entries: VecDeque::new(), capacity: capacity.max(1), next_seq: 1 }
    }

    /// Adds an entry and returns its sequence number.
    pub fn push(&mut self, r: Record<'_>) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(RingEntry {
            seq,
            t_ms: r.t_ms,
            id: crate::refusal::echo(r.id),
            args: r.args.map(args_text).unwrap_or_default(),
            source: r.source,
            outcome: r.outcome,
            duration_us: r.duration_us,
            steps: r.steps,
            frames: r.frames,
            mutating: r.mutating,
        });
        seq
    }

    /// Entries, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &RingEntry> {
        self.entries.iter()
    }

    /// Number of entries held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The newest entries, oldest first: at most `limit`, optionally only mutating ones, optionally only
    /// those after `since_seq`.
    pub fn recent(&self, limit: usize, mutating_only: bool, since_seq: Option<u64>) -> Vec<RingEntry> {
        let mut out: Vec<RingEntry> = self
            .entries
            .iter()
            .rev()
            .filter(|e| !mutating_only || e.mutating)
            .filter(|e| since_seq.is_none_or(|s| e.seq > s))
            .take(limit)
            .cloned()
            .collect();
        out.reverse();
        out
    }

    /// The sequence number the next entry gets.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// The ring's capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Compact JSON text of `v`, at most [`ARGS_MAX_BYTES`] long. Long strings (image data) are written as a
/// length. Stops writing as soon as the budget is spent, so a huge value costs a bounded amount.
pub fn args_text(v: &Value) -> String {
    let mut out = String::new();
    let _ = write_value(v, &mut out, ARGS_MAX_BYTES);
    out
}

/// Strings longer than this are replaced by their length.
const LONG_STRING: usize = 256;

fn write_value(v: &Value, out: &mut String, budget: usize) -> bool {
    if out.len() >= budget {
        return false;
    }
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) if s.len() > LONG_STRING => out.push_str(&format!("\"<{} bytes>\"", s.len())),
        Value::String(s) => out.push_str(&Value::String(s.clone()).to_string()),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if !write_value(x, out, budget) {
                    return cut(out, budget);
                }
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                if !write_value(x, out, budget) {
                    return cut(out, budget);
                }
            }
            out.push('}');
        }
    }
    if out.len() > budget {
        return cut(out, budget);
    }
    true
}

fn cut(out: &mut String, budget: usize) -> bool {
    let mut end = budget.min(out.len());
    while end > 0 && !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out.push('…');
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(id: &str, mutating: bool) -> Record<'_> {
        Record {
            t_ms: 1,
            id,
            args: None,
            source: Origin::Test,
            outcome: Outcome::ok(true, None),
            duration_us: 3,
            steps: None,
            frames: None,
            mutating,
        }
    }

    #[test]
    fn ring_is_bounded_and_sequenced() {
        let mut r = Ring::new(3);
        for i in 0..5 {
            r.push(rec(&format!("a.b{i}"), true));
        }
        let ids: Vec<&str> = r.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["a.b2", "a.b3", "a.b4"]);
        assert_eq!(r.iter().map(|e| e.seq).collect::<Vec<_>>(), [3, 4, 5]);
        assert_eq!(r.next_seq(), 6);
    }

    #[test]
    fn recent_filters_and_orders_oldest_first() {
        let mut r = Ring::new(10);
        r.push(rec("a.read", false));
        r.push(rec("a.write", true));
        r.push(rec("a.read2", false));
        r.push(rec("a.write2", true));
        let m: Vec<String> = r.recent(10, true, None).into_iter().map(|e| e.id).collect();
        assert_eq!(m, ["a.write", "a.write2"]);
        let last: Vec<String> = r.recent(2, false, None).into_iter().map(|e| e.id).collect();
        assert_eq!(last, ["a.read2", "a.write2"]);
        let since: Vec<String> = r.recent(10, false, Some(2)).into_iter().map(|e| e.id).collect();
        assert_eq!(since, ["a.read2", "a.write2"]);
    }

    #[test]
    fn args_are_truncated_and_image_bytes_become_lengths() {
        let big = json!({"bytes": {"base64": "A".repeat(100_000)}, "name": "x"});
        let t = args_text(&big);
        assert!(t.contains("<100000 bytes>"), "{t}");
        let many = json!({"ids": (0..1000).map(|i| format!("id{i}")).collect::<Vec<_>>()});
        let t = args_text(&many);
        assert!(t.len() <= ARGS_MAX_BYTES + 4, "{}", t.len());
        assert!(t.ends_with('…'));
        assert_eq!(args_text(&json!({"dx": 1})), r#"{"dx":1}"#);
    }

    #[test]
    fn origin_accepts_the_catalog_names() {
        let o: Origin = serde_json::from_str("\"keyboard\"").unwrap();
        assert_eq!(o, Origin::Shortcut);
        let o: Origin = serde_json::from_str("\"mcp\"").unwrap();
        assert_eq!(o, Origin::Agent);
    }
}
