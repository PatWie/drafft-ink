//! Reading data written by a newer version: unknown fields are set aside and counted instead of failing the read.
//!
//! The model structs refuse unknown fields (`deny_unknown_fields`), which keeps typos out of files and commands
//! and keeps the JSON Schema exact. That also makes an older build reject a file that a newer build wrote with one
//! more field. [`UnknownFields`] and [`from_value`] are the way around it for data that comes from a file or a
//! peer: read strictly, and when the only complaint is an unknown field, take that field out, note where it was,
//! and read again. Every other error is returned as it is. Arguments of commands do not go through here: an
//! unknown command argument stays an error, because there it is a typo (`docs/formats.md`, "Newer files").

use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_path_to_error::Segment;

/// Most paths [`UnknownFields`] keeps. The count goes on past it.
pub const MAX_RECORDED: usize = 64;

/// Most unknown fields taken out of one value. A value with more is refused as hostile.
const MAX_PER_VALUE: usize = 256;

/// The unknown fields met while reading, as `style.highlight` paths (an index shows as `elements[3].style.x`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnknownFields {
    paths: Vec<String>,
    total: usize,
}

impl UnknownFields {
    /// Nothing unknown yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no unknown field was met.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// How many unknown fields were met, the ones past [`MAX_RECORDED`] included.
    pub fn count(&self) -> usize {
        self.total
    }

    /// The first [`MAX_RECORDED`] paths, in the order they were met.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// The distinct last names (`highlight`), for a message or a log line.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.paths.iter().map(|p| p.rsplit('.').next().unwrap_or(p)).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Notes one more unknown field at `path`.
    pub fn record(&mut self, path: String) {
        self.total += 1;
        if self.paths.len() < MAX_RECORDED {
            self.paths.push(path);
        }
    }

    /// Adds what `other` met, under `prefix` (`elements[3]`).
    pub fn absorb(&mut self, other: UnknownFields, prefix: &str) {
        self.total += other.total;
        for p in other.paths {
            if self.paths.len() < MAX_RECORDED {
                self.paths.push(if prefix.is_empty() { p } else { format!("{prefix}.{p}") });
            }
        }
    }

    /// The sentence the app shows when a document had unknown fields.
    pub const WARNING: &'static str = "This drawing uses features from a newer version of Drafft.ink. They are hidden here and may be lost if you save.";
}

/// Reads a `T` from `value`, taking out the unknown fields that `T` refuses and noting them in `unknown`. Any other
/// problem is an error, as it would be for a strict read.
pub fn from_value<T: DeserializeOwned>(mut value: Value, unknown: &mut UnknownFields) -> Result<T, serde_json::Error> {
    for _ in 0..=MAX_PER_VALUE {
        match serde_path_to_error::deserialize::<_, T>(&value) {
            Ok(v) => return Ok(v),
            Err(e) => {
                let Some(name) = unknown_field_name(&e.inner().to_string()) else { return Err(e.into_inner()) };
                let mut segments: Vec<Segment> = e.path().iter().cloned().collect();
                // The path names the struct that refused or the unknown key itself, depending on where the reader
                // noticed; take the last segment off when it is the key.
                if matches!(segments.last(), Some(Segment::Map { key }) if *key == name) {
                    segments.pop();
                }
                let Some(below) = remove(&mut value, &segments, &name) else { return Err(e.into_inner()) };
                let found = join(&render(&segments), &below);
                unknown.record(join(&found, &name));
            }
        }
    }
    Err(serde::de::Error::custom("the value has too many unknown fields"))
}

/// `name` out of "unknown field `name`, expected ...".
fn unknown_field_name(message: &str) -> Option<String> {
    let rest = message.strip_prefix("unknown field `")?;
    let end = rest.find('`')?;
    Some(rest[..end].to_owned())
}

/// Removes `name` from the object at `segments` below `value`, or else from the first object below it that has the
/// key. The reader does not always say how deep it was: inside an internally tagged enum (`ElementKind`) the path
/// ends at the enum, and the key can sit in a list of the variant's data. Returns where the key was, relative to
/// the object the path names; `None` when there is no such key.
fn remove(value: &mut Value, segments: &[Segment], name: &str) -> Option<String> {
    let mut at = value;
    for s in segments {
        at = match (s, at) {
            (Segment::Map { key }, Value::Object(o)) => match o.get_mut(key) {
                Some(v) => v,
                None => return None,
            },
            (Segment::Seq { index }, Value::Array(a)) => match a.get_mut(*index) {
                Some(v) => v,
                None => return None,
            },
            // Enum variants and unknown steps have no level of their own in the JSON.
            (Segment::Enum { .. } | Segment::Unknown, v) => v,
            _ => return None,
        };
    }
    remove_first(at, name)
}

/// Removes the key from `at` or from what lies below it. Returns where it was, relative to `at`: `""`, `runs[0]`.
fn remove_first(at: &mut Value, name: &str) -> Option<String> {
    match at {
        Value::Object(o) => {
            if o.remove(name).is_some() {
                return Some(String::new());
            }
            o.iter_mut().find_map(|(k, v)| remove_first(v, name).map(|rest| join(k, &rest)))
        }
        Value::Array(a) => {
            a.iter_mut().enumerate().find_map(|(i, v)| remove_first(v, name).map(|rest| join(&format!("[{i}]"), &rest)))
        }
        _ => None,
    }
}

/// `a` and `b` as one path: `runs` and `[0]` make `runs[0]`, `kind` and `runs[0]` make `kind.runs[0]`.
fn join(a: &str, b: &str) -> String {
    match b {
        _ if a.is_empty() => b.to_owned(),
        "" => a.to_owned(),
        b if b.starts_with('[') => format!("{a}{b}"),
        b => format!("{a}.{b}"),
    }
}

fn render(segments: &[Segment]) -> String {
    let mut out = String::new();
    for s in segments {
        match s {
            Segment::Map { key } => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            Segment::Seq { index } => out.push_str(&format!("[{index}]")),
            Segment::Enum { .. } | Segment::Unknown => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;
    use crate::{Element, ElementKind, Style};

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Inner {
        a: u32,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Outer {
        inner: Inner,
        list: Vec<Inner>,
    }

    #[test]
    fn unknown_fields_at_every_depth_are_removed_and_named() {
        let v = json!({"inner": {"a": 1, "extra": true}, "list": [{"a": 2}, {"a": 3, "more": 1}], "top": 0});
        let mut unknown = UnknownFields::new();
        let got: Outer = from_value(v, &mut unknown).unwrap();
        assert_eq!(got, Outer { inner: Inner { a: 1 }, list: vec![Inner { a: 2 }, Inner { a: 3 }] });
        let mut paths = unknown.paths().to_vec();
        paths.sort();
        assert_eq!(paths, ["inner.extra", "list[1].more", "top"]);
        assert_eq!(unknown.count(), 3);
    }

    #[test]
    fn other_errors_are_not_swallowed() {
        let mut unknown = UnknownFields::new();
        assert!(from_value::<Inner>(json!({"a": "text", "x": 1}), &mut unknown).is_err());
        assert!(from_value::<Inner>(json!({"x": 1}), &mut unknown).is_err(), "a missing field stays an error");
    }

    #[test]
    fn a_style_and_a_tagged_kind_of_a_newer_version_load() {
        let mut unknown = UnknownFields::new();
        let v = json!({
            "id": "t1",
            "kind": {"type": "text", "content": "hi", "future_kind_field": 1},
            "transform": {"x": 0.0, "y": 0.0, "w": 10.0, "h": 10.0},
            "style": {"sparkle": "gold", "stroke_width": 3.0},
            "index": "a0",
            "future_top": true
        });
        let e: Element = from_value(v, &mut unknown).unwrap();
        assert!(matches!(e.kind, ElementKind::Text(_)));
        assert_eq!(e.style, Style { stroke_width: 3.0, ..Style::default() });
        let mut paths = unknown.paths().to_vec();
        paths.sort();
        assert_eq!(paths, ["future_top", "kind.future_kind_field", "style.sparkle"]);
    }

    #[test]
    fn a_hostile_value_with_endless_unknown_fields_is_refused() {
        let mut o = serde_json::Map::new();
        o.insert("a".into(), json!(1));
        for i in 0..400 {
            o.insert(format!("k{i}"), json!(i));
        }
        let mut unknown = UnknownFields::new();
        assert!(from_value::<Inner>(Value::Object(o), &mut unknown).is_err());
    }
}
