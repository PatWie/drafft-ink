//! The text the Copy button of an error toast puts on the clipboard: what a person pastes into a bug report.
//!
//! It holds the refusal code, the command, a short summary of the arguments, the plain-word message, and the
//! app version and platform. The summary never carries a long value (a picture, a document) and hides the
//! values of keys that look like secrets.

use serde_json::Value;

/// A string longer than this many characters is cut in the summary.
const MAX_STRING: usize = 64;
/// An array or object with more items than this is cut in the summary.
const MAX_ITEMS: usize = 8;
/// Nesting deeper than this is replaced by `…`.
const MAX_DEPTH: usize = 4;
/// Key fragments whose values are never shown.
const SECRET_KEYS: &[&str] = &["token", "secret", "password", "passwd", "auth", "cookie", "key"];

/// The arguments of a call as one short line of JSON.
pub(crate) fn summarize_args(args: &Value) -> String {
    shorten(args, 0).to_string()
}

fn shorten(value: &Value, depth: usize) -> Value {
    match value {
        _ if depth > MAX_DEPTH => Value::String("…".to_owned()),
        Value::String(s) if s.chars().count() > MAX_STRING => {
            let head: String = s.chars().take(MAX_STRING / 2).collect();
            Value::String(format!("{head}… ({} characters)", s.chars().count()))
        }
        Value::Array(items) => {
            let mut out: Vec<Value> = items.iter().take(MAX_ITEMS).map(|v| shorten(v, depth + 1)).collect();
            if items.len() > MAX_ITEMS {
                out.push(Value::String(format!("… {} more", items.len() - MAX_ITEMS)));
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out: serde_json::Map<String, Value> = map
                .iter()
                .take(MAX_ITEMS)
                .map(|(k, v)| {
                    let hidden = SECRET_KEYS.iter().any(|s| k.to_ascii_lowercase().contains(s));
                    let v = if hidden { Value::String("<hidden>".to_owned()) } else { shorten(v, depth + 1) };
                    (k.clone(), v)
                })
                .collect();
            if map.len() > MAX_ITEMS {
                out.insert("…".to_owned(), Value::String(format!("{} more", map.len() - MAX_ITEMS)));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// The operating system, or `web` in the browser.
fn platform() -> String {
    if cfg!(target_arch = "wasm32") {
        "web (wasm32)".to_owned()
    } else {
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

/// What the Copy button copies.
pub(crate) struct ErrorReport<'a> {
    /// The refusal code.
    pub code: &'a str,
    /// The command that refused (empty: a note, not a refusal).
    pub command: &'a str,
    /// The summary of its arguments.
    pub args: &'a str,
    /// The message the toast shows.
    pub message: &'a str,
    /// The app version, with the commit when it is known.
    pub version: &'a str,
}

impl ErrorReport<'_> {
    /// The report as plain text, one fact per line.
    pub(crate) fn text(&self) -> String {
        let mut lines = vec![format!("code: {}", self.code)];
        if !self.command.is_empty() {
            lines.push(format!("command: {}", self.command));
        }
        if !self.args.is_empty() {
            lines.push(format!("args: {}", self.args));
        }
        lines.push(format!("message: {}", self.message));
        lines.push(format!("version: {}", self.version));
        lines.push(format!("platform: {}", platform()));
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn long_values_are_cut_and_secrets_hidden() {
        let blob = "A".repeat(10_000);
        let s =
            summarize_args(&json!({ "data": blob, "api_token": "abc", "n": 3, "list": (0..20).collect::<Vec<_>>() }));
        assert!(s.len() < 400, "{s}");
        assert!(s.contains("10000 characters") && s.contains("<hidden>") && s.contains("12 more"), "{s}");
        assert!(!s.contains("abc"));
    }

    #[test]
    fn deep_nesting_is_cut() {
        let s = summarize_args(&json!({ "a": { "b": { "c": { "d": { "e": { "f": 1 } } } } } }));
        assert!(s.contains('…') && !s.contains("\"f\""), "{s}");
    }

    #[test]
    fn the_report_names_code_command_message_version_and_platform() {
        let text = ErrorReport {
            code: "gesture_active",
            command: "tool.set",
            args: "{}",
            message: "A drag.",
            version: "1.2.3",
        }
        .text();
        for part in [
            "code: gesture_active",
            "command: tool.set",
            "args: {}",
            "message: A drag.",
            "version: 1.2.3",
            "platform: ",
        ] {
            assert!(text.contains(part), "{part} in {text}");
        }
    }
}
