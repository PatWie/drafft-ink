//! Checks on a value: the `expect` list of a scenario step.
//!
//! A check is an object with an optional `path` (default `$`, the whole output) and exactly one operator:
//!
//! | Operator | Meaning |
//! |---|---|
//! | `"eq": v` / `"ne": v` | equal / not equal. Numbers compare as numbers (`2` equals `2.0`) |
//! | `"contains": v` | a string contains the text; an array has the item; an object has the key |
//! | `"gt"`, `"ge"`, `"lt"`, `"le": n` | numeric order |
//! | `"near": n` with `"tol": t` (default 1e-6) and optional `"plus": d` | within `t` of `n + d`. `n` may be an array of numbers, then each item is compared |
//! | `"exists": true/false` | the path leads somewhere / leads nowhere |
//! | `"len": n` | length of an array, string (in characters) or object |
//! | `"is": "string"` | the JSON type: `string`, `number`, `bool`, `array`, `object`, `null` |
//!
//! A check may also carry a `"note"`, which is shown when it fails. Expected values may use `{{name}}` to refer to a
//! value saved by an earlier step.

use serde_json::Value;

use crate::path::{self, Vars};

/// One operator with its operand.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Equal.
    Eq(Value),
    /// Not equal.
    Ne(Value),
    /// Contains.
    Contains(Value),
    /// Greater than.
    Gt(Value),
    /// Greater than or equal.
    Ge(Value),
    /// Less than.
    Lt(Value),
    /// Less than or equal.
    Le(Value),
    /// Within a tolerance.
    Near {
        /// The expected number or array of numbers.
        value: Value,
        /// Tolerance.
        tol: f64,
        /// Added to a number before comparing.
        plus: f64,
    },
    /// The path exists (or does not).
    Exists(bool),
    /// Length.
    Len(usize),
    /// JSON type name.
    Is(String),
}

/// A path and an operator.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// Where to look.
    pub path: String,
    /// What to require.
    pub op: Op,
    /// Why the check is there; shown when it fails.
    pub note: Option<String>,
}

const OPS: &[&str] = &["eq", "ne", "contains", "gt", "ge", "lt", "le", "near", "exists", "len", "is"];
const TYPES: &[&str] = &["string", "number", "bool", "array", "object", "null"];

impl Check {
    /// Reads a check from its JSON form. Unknown keys, a missing or doubled operator and bad paths are errors.
    pub fn parse(v: &Value) -> Result<Check, String> {
        let Value::Object(m) = v else {
            return Err(format!("a check is an object like {{\"path\": \"$.x\", \"eq\": 1}}, got {v}"));
        };
        let allowed = |k: &str| k == "path" || k == "tol" || k == "plus" || k == "note" || OPS.contains(&k);
        if let Some(k) = m.keys().find(|k| !allowed(k)) {
            return Err(format!("unknown key `{k}` in a check. Keys: path, {}, tol, plus, note", OPS.join(", ")));
        }
        let ops: Vec<&str> = OPS.iter().copied().filter(|o| m.contains_key(*o)).collect();
        let [op_name] = ops.as_slice() else {
            return Err(format!(
                "a check needs exactly one operator ({}), this one has {}",
                OPS.join(", "),
                if ops.is_empty() { "none".to_owned() } else { ops.join(" and ") }
            ));
        };
        if (m.contains_key("tol") || m.contains_key("plus")) && *op_name != "near" {
            return Err("`tol` and `plus` belong to `near`".to_owned());
        }
        let path = match m.get("path") {
            None => "$".to_owned(),
            Some(Value::String(p)) => p.clone(),
            Some(other) => return Err(format!("`path` must be a string, got {other}")),
        };
        path::validate(&path).map_err(|e| e.to_string())?;
        let operand = m.get(*op_name).cloned().unwrap_or(Value::Null);
        let number = |name: &str| -> Result<f64, String> {
            m.get(name).map_or(Ok(0.0), |x| x.as_f64().ok_or_else(|| format!("`{name}` must be a number, got {x}")))
        };
        let op = match *op_name {
            "eq" => Op::Eq(operand),
            "ne" => Op::Ne(operand),
            "contains" => Op::Contains(operand),
            "gt" => Op::Gt(operand),
            "ge" => Op::Ge(operand),
            "lt" => Op::Lt(operand),
            "le" => Op::Le(operand),
            "near" => {
                let tol = if m.contains_key("tol") { number("tol")? } else { 1e-6 };
                Op::Near { value: operand, tol, plus: number("plus")? }
            }
            "exists" => {
                Op::Exists(operand.as_bool().ok_or_else(|| format!("`exists` must be true or false, got {operand}"))?)
            }
            "len" => Op::Len(
                operand
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| format!("`len` must be a count, got {operand}"))?,
            ),
            _ => {
                let t = operand.as_str().ok_or_else(|| format!("`is` must be a type name, got {operand}"))?;
                if !TYPES.contains(&t) {
                    return Err(format!("`is` knows {}, not `{t}`", TYPES.join(", ")));
                }
                Op::Is(t.to_owned())
            }
        };
        let note = match m.get("note") {
            None => None,
            Some(Value::String(n)) => Some(n.clone()),
            Some(other) => return Err(format!("`note` must be a string, got {other}")),
        };
        Ok(Check { path, op, note })
    }

    /// Runs the check on `actual`. The error says what was expected and what was found, then the note.
    pub fn run(&self, actual: &Value, vars: &Vars) -> Result<(), String> {
        self.run_inner(actual, vars).map_err(|e| match &self.note {
            Some(n) => format!("{e} ({n})"),
            None => e,
        })
    }

    fn run_inner(&self, actual: &Value, vars: &Vars) -> Result<(), String> {
        let found = path::select(actual, &self.path).map_err(|e| e.to_string())?;
        let what = format!("{} {}", self.path, self.op.describe());
        if let Op::Exists(want) = &self.op {
            return if found.is_some() == *want {
                Ok(())
            } else if *want {
                Err(format!("{what}: nothing at that path"))
            } else {
                Err(format!("{what}: found {}", short(found)))
            };
        }
        let Some(got) = found else {
            return Err(format!("{what}: nothing at that path"));
        };
        let sub = |v: &Value| crate::path::substitute(v, vars).map_err(|e| format!("{what}: {e}"));
        let ok = match &self.op {
            Op::Eq(v) => equal(got, &sub(v)?),
            Op::Ne(v) => !equal(got, &sub(v)?),
            Op::Contains(v) => contains(got, &sub(v)?),
            Op::Gt(v) => order(got, &sub(v)?, &what)?.is_gt(),
            Op::Ge(v) => order(got, &sub(v)?, &what)?.is_ge(),
            Op::Lt(v) => order(got, &sub(v)?, &what)?.is_lt(),
            Op::Le(v) => order(got, &sub(v)?, &what)?.is_le(),
            Op::Near { value, tol, plus } => {
                return near(got, &sub(value)?, *tol, *plus).map_err(|e| format!("{what}: {e}"));
            }
            Op::Len(n) => length(got) == Some(*n),
            Op::Is(t) => type_name(got) == t,
            Op::Exists(_) => true,
        };
        if ok { Ok(()) } else { Err(format!("{what}: found {}", short(Some(got)))) }
    }
}

impl Op {
    fn describe(&self) -> String {
        match self {
            Op::Eq(v) => format!("eq {v}"),
            Op::Ne(v) => format!("ne {v}"),
            Op::Contains(v) => format!("contains {v}"),
            Op::Gt(v) => format!("gt {v}"),
            Op::Ge(v) => format!("ge {v}"),
            Op::Lt(v) => format!("lt {v}"),
            Op::Le(v) => format!("le {v}"),
            Op::Near { value, tol, plus } if *plus == 0.0 => format!("near {value} (tol {tol})"),
            Op::Near { value, tol, plus } => format!("near {value} + {plus} (tol {tol})"),
            Op::Exists(b) => format!("exists {b}"),
            Op::Len(n) => format!("len {n}"),
            Op::Is(t) => format!("is {t}"),
        }
    }
}

fn short(v: Option<&Value>) -> String {
    let Some(v) = v else { return "nothing".to_owned() };
    let text = v.to_string();
    if text.chars().count() > 160 { format!("{}...", text.chars().take(160).collect::<String>()) } else { text }
}

/// JSON equality where numbers compare as numbers.
pub fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| equal(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| equal(v, w)))
        }
        _ => a == b,
    }
}

fn contains(hay: &Value, needle: &Value) -> bool {
    match (hay, needle) {
        (Value::String(h), Value::String(n)) => h.contains(n.as_str()),
        (Value::Array(a), n) => a.iter().any(|x| equal(x, n)),
        (Value::Object(m), Value::String(k)) => m.contains_key(k),
        _ => false,
    }
}

fn order(a: &Value, b: &Value, what: &str) -> Result<std::cmp::Ordering, String> {
    let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) else {
        return Err(format!("{what}: both sides must be numbers, found {a} and {b}"));
    };
    x.partial_cmp(&y).ok_or_else(|| format!("{what}: cannot order {x} and {y}"))
}

fn near(got: &Value, want: &Value, tol: f64, plus: f64) -> Result<(), String> {
    match (got, want) {
        (Value::Array(g), Value::Array(w)) => {
            if g.len() != w.len() {
                return Err(format!("found {} items, expected {}", g.len(), w.len()));
            }
            for (i, (a, b)) in g.iter().zip(w).enumerate() {
                near(a, b, tol, plus).map_err(|e| format!("item {i}: {e}"))?;
            }
            Ok(())
        }
        _ => {
            let (Some(g), Some(w)) = (got.as_f64(), want.as_f64()) else {
                return Err(format!("both sides must be numbers, found {got} and {want}"));
            };
            let target = w + plus;
            if (g - target).abs() <= tol {
                Ok(())
            } else {
                Err(format!("found {g}, expected {target} within {tol} (off by {})", (g - target).abs()))
            }
        }
    }
}

fn length(v: &Value) -> Option<usize> {
    match v {
        Value::Array(a) => Some(a.len()),
        Value::String(s) => Some(s.chars().count()),
        Value::Object(m) => Some(m.len()),
        _ => None,
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(check: Value, actual: &Value) -> Result<(), String> {
        Check::parse(&check)?.run(actual, &Vars::new())
    }

    #[test]
    fn each_operator_passes_and_fails_on_the_right_values() {
        let v = json!({ "n": 2, "s": "hello world", "a": [1, 2, 3], "o": { "k": 1 }, "t": true });
        for ok in [
            json!({"path": "$.n", "eq": 2.0}),
            json!({"path": "$.n", "ne": 3}),
            json!({"path": "$.s", "contains": "lo wo"}),
            json!({"path": "$.a", "contains": 2}),
            json!({"path": "$.o", "contains": "k"}),
            json!({"path": "$.n", "gt": 1}),
            json!({"path": "$.n", "ge": 2}),
            json!({"path": "$.n", "lt": 3}),
            json!({"path": "$.n", "le": 2}),
            json!({"path": "$.n", "near": 2.0004, "tol": 0.001}),
            json!({"path": "$.n", "near": 1, "plus": 1}),
            json!({"path": "$.a", "near": [1.0, 2.0, 3.0]}),
            json!({"path": "$.zzz", "exists": false}),
            json!({"path": "$.t", "exists": true}),
            json!({"path": "$.a", "len": 3}),
            json!({"path": "$.s", "len": 11}),
            json!({"path": "$.t", "is": "bool"}),
            json!({"eq": v.clone()}),
        ] {
            assert_eq!(run(ok.clone(), &v), Ok(()), "{ok}");
        }
        for bad in [
            json!({"path": "$.n", "eq": 3}),
            json!({"path": "$.n", "ne": 2}),
            json!({"path": "$.s", "contains": "xyz"}),
            json!({"path": "$.a", "contains": 9}),
            json!({"path": "$.n", "gt": 2}),
            json!({"path": "$.n", "lt": 2}),
            json!({"path": "$.n", "near": 2.1, "tol": 0.01}),
            json!({"path": "$.a", "near": [1.0, 2.0]}),
            json!({"path": "$.n", "exists": false}),
            json!({"path": "$.zzz", "exists": true}),
            json!({"path": "$.zzz", "eq": 1}),
            json!({"path": "$.a", "len": 4}),
            json!({"path": "$.t", "is": "string"}),
        ] {
            assert!(run(bad.clone(), &v).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn failures_say_what_was_expected_and_what_was_found() {
        let e = run(json!({"path": "$.n", "eq": 3, "note": "because"}), &json!({"n": 2})).unwrap_err();
        assert!(e.ends_with("(because)"), "{e}");
        let e = run(json!({"path": "$.n", "eq": 3}), &json!({"n": 2})).unwrap_err();
        assert!(e.contains("$.n") && e.contains("eq 3") && e.contains("found 2"), "{e}");
        let e = run(json!({"path": "$.n", "near": 10, "tol": 0.5}), &json!({"n": 2})).unwrap_err();
        assert!(e.contains("off by 8"), "{e}");
    }

    #[test]
    fn saved_values_are_used_in_expected_values() {
        let mut vars = Vars::new();
        vars.insert("x0".into(), json!(100.0));
        let c = Check::parse(&json!({"path": "$.x", "near": "{{x0}}", "plus": 25, "tol": 0.5})).unwrap();
        assert_eq!(c.run(&json!({"x": 125.2}), &vars), Ok(()));
        assert!(c.run(&json!({"x": 130.0}), &vars).is_err());
        let missing = Check::parse(&json!({"eq": "{{nope}}"})).unwrap();
        assert!(missing.run(&json!(1), &vars).unwrap_err().contains("nope"));
    }

    #[test]
    fn malformed_checks_are_rejected_when_the_scenario_is_read() {
        for bad in [
            json!(3),
            json!({"path": "$.x"}),
            json!({"path": "$.x", "eq": 1, "ne": 2}),
            json!({"path": "$.x", "eq": 1, "tol": 0.1}),
            json!({"path": "x", "eq": 1}),
            json!({"path": "$.x", "equals": 1}),
            json!({"len": -1}),
            json!({"is": "str"}),
            json!({"exists": "yes"}),
        ] {
            assert!(Check::parse(&bad).is_err(), "{bad}");
        }
    }
}
