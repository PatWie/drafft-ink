//! A small JSONPath subset for scenario assertions, and `{{name}}` variable substitution.
//!
//! Paths start at `$` (the whole value). What may follow, any number of times:
//!
//! | Syntax | Meaning |
//! |---|---|
//! | `.key` or `['key']` | the member `key` of an object |
//! | `[3]`, `[-1]` | an array item, counted from the front or the back |
//! | `[id=e1]` | the first array item whose member `id` equals `e1` (a string, number or bool) |
//!
//! There are no wildcards, slices or recursive descent: a scenario that needs more should ask a command for
//! the answer. [`select`] returns `Ok(None)` when the path leads nowhere, so `exists: false` can be asserted.

use std::collections::BTreeMap;

use serde_json::Value;

/// A path that cannot be read, with the reason in plain words.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("bad path `{path}`: {why}")]
pub struct PathError {
    /// The path as written.
    pub path: String,
    /// What is wrong with it.
    pub why: String,
}

enum Seg {
    Key(String),
    Index(i64),
    Find(String, String),
}

fn parse(path: &str) -> Result<Vec<Seg>, PathError> {
    let bad = |why: &str| PathError { path: path.to_owned(), why: why.to_owned() };
    let Some(rest) = path.strip_prefix('$') else {
        return Err(bad("a path starts with `$`"));
    };
    let mut segs = Vec::new();
    let mut chars = rest.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '.' => {
                let start = i + 1;
                let mut end = start;
                while let Some(&(j, d)) = chars.peek() {
                    if d == '.' || d == '[' {
                        break;
                    }
                    end = j + d.len_utf8();
                    chars.next();
                }
                let key = rest.get(start..end).unwrap_or_default();
                if key.is_empty() {
                    return Err(bad("empty key after `.`"));
                }
                segs.push(Seg::Key(key.to_owned()));
            }
            '[' => {
                let start = i + 1;
                let mut end = None;
                for (j, d) in chars.by_ref() {
                    if d == ']' {
                        end = Some(j);
                        break;
                    }
                }
                let Some(end) = end else { return Err(bad("`[` without `]`")) };
                let inner = rest.get(start..end).unwrap_or_default();
                if let Some(q) = inner.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
                    segs.push(Seg::Key(q.to_owned()));
                } else if let Ok(n) = inner.parse::<i64>() {
                    segs.push(Seg::Index(n));
                } else if let Some((k, v)) = inner.split_once('=') {
                    if k.is_empty() {
                        return Err(bad("`[key=value]` needs a key"));
                    }
                    segs.push(Seg::Find(k.to_owned(), v.to_owned()));
                } else {
                    return Err(bad("brackets hold a number, a quoted key or `key=value`"));
                }
            }
            _ => return Err(bad("expected `.` or `[`")),
        }
    }
    Ok(segs)
}

fn same(v: &Value, text: &str) -> bool {
    match v {
        Value::String(s) => s == text,
        Value::Number(n) => n.to_string() == text,
        Value::Bool(b) => b.to_string() == text,
        _ => false,
    }
}

/// Checks that `path` is well formed, without reading anything.
pub fn validate(path: &str) -> Result<(), PathError> {
    parse(path).map(|_| ())
}

/// The value at `path`, or `None` when the path leads nowhere.
pub fn select<'a>(root: &'a Value, path: &str) -> Result<Option<&'a Value>, PathError> {
    let mut cur = root;
    for seg in parse(path)? {
        let next = match (&seg, cur) {
            (Seg::Key(k), Value::Object(m)) => m.get(k),
            (Seg::Index(n), Value::Array(a)) => {
                let i = if *n < 0 { a.len().checked_sub(n.unsigned_abs() as usize) } else { usize::try_from(*n).ok() };
                i.and_then(|i| a.get(i))
            }
            (Seg::Find(k, v), Value::Array(a)) => a.iter().find(|x| x.get(k).is_some_and(|f| same(f, v))),
            _ => None,
        };
        match next {
            Some(v) => cur = v,
            None => return Ok(None),
        }
    }
    Ok(Some(cur))
}

/// Variables saved by earlier steps.
pub type Vars = BTreeMap<String, Value>;

/// Replaces `{{name}}` in every string of `v`. A string that is exactly `{{name}}` becomes the saved value
/// with its type; inside a longer string the value is written as text. An unknown name is an error.
pub fn substitute(v: &Value, vars: &Vars) -> Result<Value, String> {
    Ok(match v {
        Value::String(s) => substitute_str(s, vars)?,
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, vars)).collect::<Result<_, _>>()?),
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, x)| Ok((k.clone(), substitute(x, vars)?))).collect::<Result<_, String>>()?)
        }
        other => other.clone(),
    })
}

fn lookup<'a>(name: &str, vars: &'a Vars) -> Result<&'a Value, String> {
    vars.get(name).ok_or_else(|| {
        let known: Vec<&str> = vars.keys().map(String::as_str).collect();
        format!(
            "no saved value `{name}` (saved so far: {})",
            if known.is_empty() { "none".to_owned() } else { known.join(", ") }
        )
    })
}

fn substitute_str(s: &str, vars: &Vars) -> Result<Value, String> {
    if let Some(name) = s.strip_prefix("{{").and_then(|r| r.strip_suffix("}}"))
        && !name.contains("{{")
        && !name.contains("}}")
    {
        return lookup(name.trim(), vars).cloned();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find("{{") {
        out.push_str(rest.get(..open).unwrap_or_default());
        let after = rest.get(open + 2..).unwrap_or_default();
        let Some(close) = after.find("}}") else {
            return Err(format!("`{s}` has `{{{{` without `}}}}`"));
        };
        let name = after.get(..close).unwrap_or_default().trim();
        match lookup(name, vars)? {
            Value::String(t) => out.push_str(t),
            other => out.push_str(&other.to_string()),
        }
        rest = after.get(close + 2..).unwrap_or_default();
    }
    out.push_str(rest);
    Ok(Value::String(out))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn doc() -> Value {
        json!({ "a": { "b": [10, 20, 30] }, "items": [ { "id": "e1", "x": 1 }, { "id": "e2", "x": 2 }, { "id": 7, "x": 3 } ], "k.dot": true })
    }

    #[test]
    fn keys_indexes_and_negative_indexes() {
        let d = doc();
        assert_eq!(select(&d, "$.a.b[1]").unwrap(), Some(&json!(20)));
        assert_eq!(select(&d, "$.a.b[-1]").unwrap(), Some(&json!(30)));
        assert_eq!(select(&d, "$.a.b[-3]").unwrap(), Some(&json!(10)));
        assert_eq!(select(&d, "$['k.dot']").unwrap(), Some(&json!(true)));
        assert_eq!(select(&d, "$").unwrap(), Some(&d));
    }

    #[test]
    fn a_path_that_leads_nowhere_is_none_not_an_error() {
        let d = doc();
        assert_eq!(select(&d, "$.a.zzz").unwrap(), None);
        assert_eq!(select(&d, "$.a.b[3]").unwrap(), None);
        assert_eq!(select(&d, "$.a.b[-4]").unwrap(), None);
        assert_eq!(select(&d, "$.a.b.c").unwrap(), None);
        assert_eq!(select(&d, "$.items[id=nope]").unwrap(), None);
    }

    #[test]
    fn find_selects_by_member_value() {
        let d = doc();
        assert_eq!(select(&d, "$.items[id=e2].x").unwrap(), Some(&json!(2)));
        assert_eq!(select(&d, "$.items[id=7].x").unwrap(), Some(&json!(3)));
    }

    #[test]
    fn malformed_paths_are_errors_with_the_reason() {
        let d = doc();
        for bad in ["a.b", "$.", "$.a[", "$.a[x]", "$a", "$.a..b", "$[=1]"] {
            let e = select(&d, bad).unwrap_err();
            assert_eq!(e.path, bad);
            assert!(!e.why.is_empty(), "{bad}");
        }
    }

    #[test]
    fn substitution_keeps_types_for_whole_strings_and_writes_text_inside() {
        let mut vars = Vars::new();
        vars.insert("n".into(), json!(5));
        vars.insert("id".into(), json!("e3"));
        let v = json!({ "ref": "{{id}}", "count": "{{n}}", "text": "arrow {{id}} #{{ n }}", "list": ["{{id}}", 1], "plain": "x" });
        assert_eq!(
            substitute(&v, &vars).unwrap(),
            json!({ "ref": "e3", "count": 5, "text": "arrow e3 #5", "list": ["e3", 1], "plain": "x" })
        );
    }

    #[test]
    fn unknown_variable_names_what_is_known() {
        let mut vars = Vars::new();
        vars.insert("a".into(), json!(1));
        let e = substitute(&json!("{{b}}"), &vars).unwrap_err();
        assert!(e.contains("`b`") && e.contains('a'), "{e}");
        assert!(substitute(&json!("x {{b"), &vars).is_err());
    }
}
