//! Counting the tests that skipped themselves.
//!
//! A test that cannot run here (no GPU, no display, no old app) prints a line that starts with `SKIPPED:`
//! (`crates/profile/src/skip.rs`) and passes. Nextest keeps the output of passing tests in the JUnit file of
//! the `ci` profile (`.config/nextest.toml`), and this module counts those lines so that `cargo xtask test`
//! and `cargo xtask ci` can say how many tests did not really run.

use std::collections::BTreeMap;

/// The word every skip line starts with.
pub const MARKER: &str = "SKIPPED:";

/// True when the environment variable `name` is set to something other than the empty string and `0`
/// (the rule of `drafftink_profile::env_flag`, which xtask does not link).
pub fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0")
}

/// A group of skips with one reason.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SkipGroup {
    /// A short word for the reason.
    pub kind: &'static str,
    /// How many tests.
    pub count: usize,
}

fn kind_of(reason: &str) -> &'static str {
    let lower = reason.to_lowercase();
    // The e2e message also mentions the GPU ("it needs a display and a GPU"), so it is tested first.
    if lower.contains("e2e") || lower.contains("not run here") {
        "e2e"
    } else if lower.contains("gpu") || lower.contains("adapter") {
        "gpu"
    } else if lower.contains("old app") || lower.contains("old repo") {
        "old repo"
    } else {
        "other"
    }
}

/// The first line of a captured output follows the tag that opens it in the JUnit file.
fn strip_xml_openers(mut line: &str) -> &str {
    while let Some(rest) = ["<![CDATA[", "<system-out>", "<system-err>"].iter().find_map(|tag| line.strip_prefix(tag)) {
        line = rest;
    }
    line
}

/// The skip lines in a text (the JUnit file, or the output of a test run), grouped by reason, sorted by kind.
pub fn count(text: &str) -> Vec<SkipGroup> {
    let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = strip_xml_openers(line.trim_start()).strip_prefix(MARKER) else { continue };
        *by_kind.entry(kind_of(rest)).or_default() += 1;
    }
    by_kind.into_iter().map(|(kind, count)| SkipGroup { kind, count }).collect()
}

/// `skipped 31 (e2e 13, gpu 18)`, or `skipped 0`.
pub fn describe(groups: &[SkipGroup]) -> String {
    let total: usize = groups.iter().map(|g| g.count).sum();
    if total == 0 {
        return "skipped 0".to_owned();
    }
    let parts: Vec<String> = groups.iter().map(|g| format!("{} {}", g.kind, g.count)).collect();
    format!("skipped {total} ({})", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_lines_are_counted_by_reason() {
        let text = "ok\nSKIPPED: a: no GPU adapter on this machine\n  SKIPPED: b: no GPU adapter\n\
                    SKIPPED: c: not run here; `cargo xtask e2e` runs the native suite (it needs a display and a GPU)\nSKIPPED: d: odd\nnot SKIPPED: e\n";
        let groups = count(text);
        let xml = "<system-err><![CDATA[SKIPPED: x: no GPU\n]]></system-err>";
        assert_eq!(describe(&count(xml)), "skipped 1 (gpu 1)");
        assert_eq!(describe(&groups), "skipped 4 (e2e 1, gpu 2, other 1)");
    }

    #[test]
    fn no_skips_say_so() {
        assert_eq!(describe(&count("all good\n")), "skipped 0");
    }
}
