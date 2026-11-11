//! Refusals: a command that cannot run answers with a value, never a panic and never a silent no-op.
//!
//! A [`Refusal`] has a stable `code`, plain-words `message` that says how to proceed, optional
//! machine-readable [`Suggestion`]s and optional `details`. The same text goes to a toast, to the greyed
//! palette item and to the agent (docs/commands.md §1.5). The shared codes and their exact wording live
//! here; a command-specific code is built with [`Refusal::custom`] and listed in the command's
//! `refusals` field.

use serde::Serialize;
use serde_json::{Value, json};

/// A machine-readable next step attached to a refusal.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    /// The command to run.
    pub command: String,
    /// Arguments to run it with, when they are known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
}

impl Suggestion {
    /// A suggestion that names a command and its arguments.
    pub fn new(command: impl Into<String>, args: Option<Value>) -> Self {
        Self { command: command.into(), args }
    }
}

/// Why a command did not run (or did not finish). The state is unchanged after a refusal.
#[derive(Debug, Clone, PartialEq, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct Refusal {
    /// Stable snake_case code; shared ones are in [`SHARED_CODES`].
    pub code: &'static str,
    /// The id of the command that refused. The executor fills it in when a handler leaves it empty.
    pub command: String,
    /// Plain words, one to three sentences, saying how to proceed.
    pub message: String,
    /// Next steps a UI can offer as buttons.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggest: Vec<Suggestion>,
    /// Extra data such as offending ids or a parse position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// One shared refusal code with the template of its message (docs/commands.md §1.5).
#[derive(Debug, Clone, Copy)]
pub struct CodeInfo {
    /// The code.
    pub code: &'static str,
    /// The wording, with `{placeholders}`.
    pub template: &'static str,
}

/// The shared catalog of §1.5. `refusal_codes_are_catalogued` checks every declared code against it.
pub const SHARED_CODES: &[CodeInfo] = &[
    CodeInfo {
        code: "unknown_command",
        template: "Unknown command `{id}`. Did you mean `{a}` or `{b}`? List commands with `engine.commands`.",
    },
    CodeInfo {
        code: "bad_args",
        template: "Invalid arguments for `{id}`: {detail}. See `engine.describe` for the schema.",
    },
    CodeInfo {
        code: "internal",
        template: "`{id}` failed unexpectedly and was rolled back, so the document is unchanged. Details: {msg}.",
    },
    CodeInfo {
        code: "no_target",
        template: "Nothing to act on: no elements are selected. Select elements first, or pass `ids`.",
    },
    CodeInfo {
        code: "not_found",
        template: "No element matches `{ref}`. Look up ids with `element.query` or `doc.outline`.",
    },
    CodeInfo {
        code: "locked",
        template: "Element `{id}` is locked. Unlock it first with `element.lock` (locked=false) or `element.unlock_all`.",
    },
    CodeInfo {
        code: "view_only",
        template: "The document is view-only, so `{id}` cannot change it. Turn view-only off with `view.set_view_only` (on=false).",
    },
    CodeInfo {
        code: "gesture_active",
        template: "A drag is in progress. Finish it with `gesture.end` or cancel it with `gesture.cancel` before running `{id}`.",
    },
    CodeInfo {
        code: "text_editing",
        template: "A text element is being edited. Finish with `text.edit_end` before running `{id}`.",
    },
    CodeInfo {
        code: "unsaved_changes",
        template: "This document has unsaved changes. Save it with `doc.save`, or pass on_unsaved=\"discard\" to drop them, or on_unsaved=\"save\" to save and continue.",
    },
    CodeInfo {
        code: "newer_schema",
        template: "This document was changed by a newer version of drafft. Update the app to edit it.",
    },
    CodeInfo { code: "wrong_kind", template: "`{id}` does not work on {kind} elements. It works on {allowed}." },
    CodeInfo {
        code: "not_applicable",
        template: "None of the targeted elements support {props}. They apply to {kinds}.",
    },
    CodeInfo {
        code: "too_few",
        template: "`{id}` needs at least {n} {things}, and {have} were given. Select more, or pass `ids`.",
    },
    CodeInfo {
        code: "port_unavailable",
        template: "`{id}` needs {port}, which this session does not provide ({where}). {alternative}",
    },
    CodeInfo {
        code: "io_failed",
        template: "Could not {read|write} {target}: {msg}. Check the location and permissions, or pass the data as `bytes`.",
    },
    CodeInfo { code: "parse_failed", template: "{what} could not be read: {msg}. {hint}" },
    CodeInfo {
        code: "limit_exceeded",
        template: "`{id}` was given {n} {things}, and the limit is {max}. Split the work into several calls.",
    },
    CodeInfo {
        code: "needs_single",
        template: "`{id}` needs exactly one {thing} selected, and {have} are selected. Select one {thing} first, or pass it by id.",
    },
    CodeInfo { code: "nothing_to_do", template: "Nothing to {verb}: {reason}." },
    CodeInfo { code: "not_supported", template: "`{id}` is registered but not available yet: {reason}." },
];

/// Codes that a shell shows without a toast when they come from a keyboard shortcut (§1.5).
pub const QUIET_FROM_KEYBOARD: &[&str] = &["no_target", "nothing_to_do", "not_applicable"];

/// True when `code` is one of the shared codes of §1.5.
pub fn is_shared_code(code: &str) -> bool {
    SHARED_CODES.iter().any(|c| c.code == code)
}

/// Longest piece of a caller-supplied string that is echoed in a message.
const ECHO_MAX: usize = 60;

/// `s` cut to [`ECHO_MAX`] characters with an ellipsis, so that a hostile 5000-character key cannot
/// bloat a message.
pub fn echo(s: &str) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(ECHO_MAX).collect();
    if it.next().is_some() { format!("{head}…") } else { head }
}

/// `` `a` ``, `` `a` or `b` ``, `` `a`, `b` or `c` ``.
fn or_list(items: &[String]) -> String {
    let q: Vec<String> = items.iter().map(|s| format!("`{s}`")).collect();
    match q.as_slice() {
        [] => String::new(),
        [a] => a.clone(),
        [init @ .., last] => format!("{} or {}", init.join(", "), last),
    }
}

impl Refusal {
    /// A refusal with a code that is not in the shared catalog. The command lists the code in its
    /// `refusals`, and the code appears in the command's table in docs/commands.md.
    pub fn custom(code: &'static str, command: &str, message: impl Into<String>) -> Self {
        Self { code, command: command.to_owned(), message: message.into(), suggest: Vec::new(), details: None }
    }

    /// Adds a suggested next step.
    #[must_use]
    pub fn suggesting(mut self, command: impl Into<String>, args: Option<Value>) -> Self {
        self.suggest.push(Suggestion::new(command, args));
        self
    }

    /// Attaches details.
    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Sets the command id, replacing the one the constructor used. For a refusal about another id (an
    /// unknown command named in the arguments) raised by the command that was asked.
    #[must_use]
    pub fn raised_by(mut self, id: &str) -> Self {
        self.command = id.to_owned();
        self
    }

    /// Sets the command id when the handler did not.
    #[must_use]
    pub fn for_command(mut self, id: &str) -> Self {
        if self.command.is_empty() {
            self.command = id.to_owned();
        }
        self
    }

    fn shared(code: &'static str, command: &str, message: String) -> Self {
        Self::custom(code, command, message)
    }

    /// `unknown_command`, with up to three suggestions of real ids.
    pub fn unknown_command(id: &str, suggestions: &[String]) -> Self {
        let shown = echo(id);
        let message = if suggestions.is_empty() {
            format!("Unknown command `{shown}`. List commands with `engine.commands`.")
        } else {
            format!(
                "Unknown command `{shown}`. Did you mean {}? List commands with `engine.commands`.",
                or_list(suggestions)
            )
        };
        let mut r = Self::shared("unknown_command", id, message);
        r.suggest = suggestions.iter().map(|s| Suggestion::new(s.clone(), None)).collect();
        r
    }

    /// `bad_args`: the arguments do not fit the schema. `detail` names the key and what is wrong.
    pub fn bad_args(id: &str, detail: impl AsRef<str>) -> Self {
        Self::shared(
            "bad_args",
            id,
            format!("Invalid arguments for `{id}`: {}. See `engine.describe` for the schema.", detail.as_ref()),
        )
        .suggesting("engine.describe", Some(json!({ "id": id })))
    }

    /// `bad_args` for one named key, with the key in `details`.
    pub fn bad_arg(id: &str, key: &str, problem: impl AsRef<str>) -> Self {
        Self::bad_args(id, format!("`{}`: {}", echo(key), problem.as_ref())).with_details(json!({ "key": echo(key) }))
    }

    /// `internal`: a panic was caught or an invariant broke; the state was rolled back.
    pub fn internal(id: &str, msg: impl AsRef<str>) -> Self {
        Self::shared(
            "internal",
            id,
            format!(
                "`{id}` failed unexpectedly and was rolled back, so the document is unchanged. Details: {}.",
                echo_long(msg.as_ref())
            ),
        )
    }

    /// `no_target`: a command that needs elements has none.
    pub fn no_target(id: &str) -> Self {
        Self::shared(
            "no_target",
            id,
            "Nothing to act on: no elements are selected. Select elements first, or pass `ids`.".to_owned(),
        )
    }

    /// `not_found`: a reference matches no element.
    pub fn not_found(id: &str, reference: &str) -> Self {
        Self::shared(
            "not_found",
            id,
            format!("No element matches `{}`. Look up ids with `element.query` or `doc.outline`.", echo(reference)),
        )
        .with_details(json!({ "ref": echo(reference) }))
    }

    /// `not_found` for a filter that matches nothing. `filter` says what the filter asked for.
    pub fn no_match(id: &str, filter: &str) -> Self {
        Self::shared(
            "not_found",
            id,
            format!(
                "No element matches the filter ({}). Loosen the filter, or look up ids with `element.query` or `doc.outline`.",
                echo(filter)
            ),
        )
        .with_details(json!({ "filter": echo(filter) }))
    }

    /// `locked`: the element is locked.
    pub fn locked(id: &str, element: &str) -> Self {
        Self::shared(
            "locked",
            id,
            format!(
                "Element `{}` is locked. Unlock it first with `element.lock` (locked=false) or `element.unlock_all`.",
                echo(element)
            ),
        )
        .with_details(json!({ "ids": [echo(element)] }))
        .suggesting("element.unlock_all", None)
    }

    /// `view_only`: the document is view-only.
    pub fn view_only(id: &str) -> Self {
        Self::shared(
            "view_only",
            id,
            format!(
                "The document is view-only, so `{id}` cannot change it. Turn view-only off with `view.set_view_only` (on=false)."
            ),
        )
        .suggesting("view.set_view_only", Some(json!({ "on": false })))
    }

    /// `gesture_active`: a drag is running.
    pub fn gesture_active(id: &str) -> Self {
        Self::shared(
            "gesture_active",
            id,
            format!(
                "A drag is in progress. Finish it with `gesture.end` or cancel it with `gesture.cancel` before running `{id}`."
            ),
        )
        .suggesting("gesture.cancel", None)
    }

    /// `text_editing`: a text element is being edited.
    pub fn text_editing(id: &str) -> Self {
        Self::shared(
            "text_editing",
            id,
            format!("A text element is being edited. Finish with `text.edit_end` before running `{id}`."),
        )
        .suggesting("text.edit_end", Some(json!({ "commit": true })))
    }

    /// `unsaved_changes`: replacing the document would lose changes.
    pub fn unsaved_changes(id: &str) -> Self {
        Self::shared(
            "unsaved_changes",
            id,
            "This document has unsaved changes. Save it with `doc.save`, or pass on_unsaved=\"discard\" to drop them, or on_unsaved=\"save\" to save and continue."
                .to_owned(),
        )
        .suggesting("doc.save", None)
    }

    /// `newer_schema`: a newer version of the app changed the document, so this build holds it read-only.
    pub fn newer_schema(id: &str) -> Self {
        Self::shared(
            "newer_schema",
            id,
            "This document was changed by a newer version of drafft. Update the app to edit it.".to_owned(),
        )
    }

    /// `wrong_kind`: the command does not work on this kind of element.
    pub fn wrong_kind(id: &str, kind: &str, allowed: &str) -> Self {
        Self::shared("wrong_kind", id, format!("`{id}` does not work on {kind} elements. It works on {allowed}."))
    }

    /// `not_applicable`: none of the targets support the properties.
    pub fn not_applicable(id: &str, props: &str, kinds: &str) -> Self {
        Self::shared(
            "not_applicable",
            id,
            format!("None of the targeted elements support {props}. They apply to {kinds}."),
        )
    }

    /// `too_few`: the command needs more targets.
    pub fn too_few(id: &str, n: usize, things: &str, have: usize) -> Self {
        Self::shared(
            "too_few",
            id,
            format!("`{id}` needs at least {n} {things}, and {have} were given. Select more, or pass `ids`."),
        )
    }

    /// `port_unavailable`: the session lacks a port the command needs.
    pub fn port_unavailable(id: &str, port: &str, whence: &str, alternative: &str) -> Self {
        Self::shared(
            "port_unavailable",
            id,
            format!("`{id}` needs {port}, which this session does not provide ({whence}). {alternative}"),
        )
    }

    /// `io_failed`: a port read or write failed.
    pub fn io_failed(id: &str, verb: &str, target: &str, msg: &str) -> Self {
        Self::shared(
            "io_failed",
            id,
            format!(
                "Could not {verb} {}: {}. Check the location and permissions, or pass the data as `bytes`.",
                echo(target),
                echo_long(msg)
            ),
        )
    }

    /// `io_failed` for a document write that did not succeed. `target` is a phrase such as `` `plan.json` ``.
    /// The text says that the document was NOT saved and how to go on.
    pub fn not_saved(id: &str, target: &str, msg: &str) -> Self {
        Self::shared(
            "io_failed",
            id,
            format!(
                "The document was NOT saved to {target}: {}. It is still unsaved. Check the location and permissions and run `doc.save` again, or pass another place with `to`.",
                echo_long(msg)
            ),
        )
    }

    /// `parse_failed`: input could not be read.
    pub fn parse_failed(id: &str, what: &str, msg: &str, hint: &str) -> Self {
        Self::shared("parse_failed", id, format!("{what} could not be read: {}. {hint}", echo_long(msg)))
    }

    /// `limit_exceeded`: too many items.
    pub fn limit_exceeded(id: &str, n: usize, things: &str, max: usize) -> Self {
        Self::shared(
            "limit_exceeded",
            id,
            format!("`{id}` was given {n} {things}, and the limit is {max}. Split the work into several calls."),
        )
    }

    /// `needs_single`: exactly one target is needed.
    pub fn needs_single(id: &str, thing: &str, have: usize) -> Self {
        Self::shared(
            "needs_single",
            id,
            format!(
                "`{id}` needs exactly one {thing} selected, and {have} are selected. Select one {thing} first, or pass it by id."
            ),
        )
    }

    /// `nothing_to_do`: the whole purpose of the command would be void.
    pub fn nothing_to_do(id: &str, verb: &str, reason: &str) -> Self {
        Self::shared("nothing_to_do", id, format!("Nothing to {verb}: {reason}."))
    }

    /// `not_supported`: registered but planned.
    pub fn not_supported(id: &str, reason: &str) -> Self {
        Self::shared("not_supported", id, format!("`{id}` is registered but not available yet: {reason}."))
    }
}

/// Like [`echo`] with room for a parser message.
fn echo_long(s: &str) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(300).collect();
    if it.next().is_some() { format!("{head}…") } else { head }
}

/// True when the characters of `needle` appear in `hay` in order.
fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|n| it.any(|h| h == n))
}

/// Edit distance between two strings, by characters. Inputs are cut to 64 characters.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().take(64).collect();
    let b: Vec<char> = b.chars().take(64).collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev.get(j).copied().unwrap_or(0) + usize::from(ca != cb);
            let del = prev.get(j + 1).copied().unwrap_or(0) + 1;
            let ins = cur.get(j).copied().unwrap_or(0) + 1;
            cur.push(sub.min(del).min(ins));
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

/// Up to `max` of `candidates` that look like `wanted`: first those with the same last segment (after the
/// final dot), then by edit distance. A wanted string longer than 64 characters gets no hint.
pub fn closest<'a>(wanted: &str, candidates: impl IntoIterator<Item = &'a str>, max: usize) -> Vec<String> {
    if wanted.chars().count() > 64 || wanted.is_empty() {
        return Vec::new();
    }
    let wanted_lc = wanted.to_ascii_lowercase();
    let last = wanted_lc.rsplit('.').next().unwrap_or("");
    let limit = (wanted_lc.chars().count() / 3).max(2);
    let mut scored: Vec<(usize, usize, &str)> = Vec::new();
    for c in candidates {
        let c_last = c.rsplit('.').next().unwrap_or("");
        let same_last = !last.is_empty() && c_last == last;
        let d = edit_distance(&wanted_lc, c);
        let d_last = edit_distance(last, c_last);
        // A typo that dropped letters ("zoom_fit" for "zoom_to_fit") is a stronger hint than a short edit distance.
        let subseq = last.len() > 3 && is_subsequence(last, c_last);
        if same_last || subseq || d <= limit || (d_last <= 1 && last.len() > 3) {
            let tier = if same_last {
                0
            } else if subseq {
                1
            } else {
                2
            };
            scored.push((tier, d, c));
        }
    }
    scored.sort();
    scored.dedup();
    scored.into_iter().take(max).map(|(_, _, c)| c.to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_names_close_ids() {
        let ids = ["view.pan", "view.set", "doc.open", "panel.open"];
        let near = closest("file.open", ids, 3);
        assert_eq!(near.first().map(String::as_str), Some("doc.open"));
        let r = Refusal::unknown_command("file.open", &near);
        assert!(r.message.contains("Did you mean `doc.open`"), "{}", r.message);
        assert_eq!(r.code, "unknown_command");
    }

    #[test]
    fn huge_ids_get_no_hint_and_a_short_message() {
        let long = "x".repeat(10_000);
        assert!(closest(&long, ["view.pan"], 3).is_empty());
        let r = Refusal::unknown_command(&long, &[]);
        assert!(r.message.chars().count() < 200);
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("zoom", "zom"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("same", "same"), 0);
    }

    #[test]
    fn every_constructor_uses_a_shared_code() {
        let all = [
            Refusal::unknown_command("x.y", &[]),
            Refusal::bad_args("x.y", "d"),
            Refusal::internal("x.y", "m"),
            Refusal::no_target("x.y"),
            Refusal::not_found("x.y", "r"),
            Refusal::locked("x.y", "e"),
            Refusal::view_only("x.y"),
            Refusal::gesture_active("x.y"),
            Refusal::text_editing("x.y"),
            Refusal::unsaved_changes("x.y"),
            Refusal::newer_schema("x.y"),
            Refusal::wrong_kind("x.y", "a", "b"),
            Refusal::not_applicable("x.y", "a", "b"),
            Refusal::too_few("x.y", 2, "elements", 1),
            Refusal::port_unavailable("x.y", "p", "w", "a"),
            Refusal::io_failed("x.y", "read", "t", "m"),
            Refusal::parse_failed("x.y", "w", "m", "h"),
            Refusal::limit_exceeded("x.y", 2, "t", 1),
            Refusal::needs_single("x.y", "t", 2),
            Refusal::nothing_to_do("x.y", "v", "r"),
            Refusal::not_supported("x.y", "r"),
        ];
        for r in &all {
            assert!(is_shared_code(r.code), "{}", r.code);
        }
        assert_eq!(all.len(), SHARED_CODES.len(), "a shared code has no constructor");
    }
}
