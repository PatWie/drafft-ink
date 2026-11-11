//! Reading a command script.
//!
//! Four spellings are accepted, so a script can be written by hand or generated:
//!
//! * JSON Lines: one `{"id": "element.create", "args": {...}}` per line. Blank lines are skipped.
//! * A JSON array of such steps.
//! * One such step.
//! * A batch: `{"steps": [...], "opts": {...}}`, where a step may be named with `"as"` and later steps may
//!   refer to it as `"$name"`. This is exactly what `Session::execute_batch_json` takes.

use drafftink_engine::BatchStep;
use serde_json::{Value, json};

/// A script, read.
#[derive(Debug, Clone, PartialEq)]
pub enum Script {
    /// Steps to run one after the other.
    Steps(Vec<BatchStep>),
    /// A batch object, run as one transaction.
    Batch(Value),
}

/// Why a script could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScriptError {
    /// Nothing in it.
    #[error("the script is empty. Give one command per line, like {{\"id\": \"element.create\", \"args\": {{...}}}}")]
    Empty,
    /// A JSON value that is none of the spellings.
    #[error(
        "the script is a JSON {0}, and a script is an array of steps, a batch object with `steps`, or one step per line"
    )]
    WrongShape(&'static str),
    /// A step that does not have the shape of a step.
    #[error("{place}: {message}. A step is {{\"id\": \"<command id>\", \"args\": {{...}}}}")]
    BadStep {
        /// Where: a line or a position.
        place: String,
        /// What serde said.
        message: String,
    },
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn step(value: Value, place: String) -> Result<BatchStep, ScriptError> {
    serde_json::from_value(value).map_err(|e| ScriptError::BadStep { place, message: e.to_string() })
}

impl Script {
    /// Reads a script from text.
    pub fn parse(text: &str) -> Result<Self, ScriptError> {
        if text.trim().is_empty() {
            return Err(ScriptError::Empty);
        }
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(items)) => items
                .into_iter()
                .enumerate()
                .map(|(i, v)| step(v, format!("step {}", i + 1)))
                .collect::<Result<_, _>>()
                .map(Self::Steps),
            Ok(Value::Object(map)) if map.contains_key("steps") => Ok(Self::Batch(Value::Object(map))),
            Ok(v @ Value::Object(_)) => Ok(Self::Steps(vec![step(v, "the step".to_owned())?])),
            Ok(other) => Err(ScriptError::WrongShape(kind_of(&other))),
            // Not one JSON value: JSON Lines.
            Err(_) => text
                .lines()
                .enumerate()
                .filter(|(_, l)| !l.trim().is_empty())
                .map(|(i, l)| {
                    let place = format!("line {}", i + 1);
                    let v: Value = serde_json::from_str(l)
                        .map_err(|e| ScriptError::BadStep { place: place.clone(), message: e.to_string() })?;
                    step(v, place)
                })
                .collect::<Result<_, _>>()
                .map(Self::Steps),
        }
    }

    /// The script as one batch object. A batch stays as it is; steps get the options given.
    pub fn into_batch(self, label: Option<&str>, dry_run: bool) -> Value {
        match self {
            Self::Batch(v) => v,
            Self::Steps(steps) => json!({
                "steps": steps,
                "opts": { "label": label, "dry_run": dry_run },
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(s: &Script) -> Vec<&str> {
        match s {
            Script::Steps(v) => v.iter().map(|s| s.id.as_str()).collect(),
            Script::Batch(_) => vec!["<batch>"],
        }
    }

    #[test]
    fn json_lines_an_array_and_one_step_read_the_same() {
        let lines = "{\"id\":\"a.b\"}\n\n{\"id\":\"c.d\",\"args\":{\"x\":1}}\n";
        let array = "[{\"id\":\"a.b\"},{\"id\":\"c.d\",\"args\":{\"x\":1}}]";
        assert_eq!(Script::parse(lines).unwrap(), Script::parse(array).unwrap());
        assert_eq!(ids(&Script::parse(lines).unwrap()), ["a.b", "c.d"]);
        assert_eq!(ids(&Script::parse("{\"id\":\"a.b\"}").unwrap()), ["a.b"]);
        assert_eq!(ids(&Script::parse("{\n  \"id\": \"a.b\",\n  \"args\": {}\n}").unwrap()), ["a.b"], "pretty printed");
    }

    #[test]
    fn an_object_with_steps_is_a_batch() {
        let s = Script::parse("{\"steps\":[{\"id\":\"a.b\",\"as\":\"x\"}],\"opts\":{\"atomic\":false}}").unwrap();
        assert!(matches!(&s, Script::Batch(v) if v["opts"]["atomic"] == false));
        assert_eq!(
            s.clone().into_batch(Some("ignored"), true),
            match s {
                Script::Batch(v) => v,
                Script::Steps(_) => unreachable!(),
            }
        );
    }

    #[test]
    fn steps_become_a_batch_with_the_options() {
        let b = Script::parse("{\"id\":\"a.b\",\"as\":\"x\"}").unwrap().into_batch(Some("my label"), true);
        assert_eq!(b["steps"][0]["as"], "x");
        assert_eq!(b["opts"]["label"], "my label");
        assert_eq!(b["opts"]["dry_run"], true);
    }

    #[test]
    fn mistakes_say_where_they_are() {
        assert_eq!(Script::parse("  \n ").unwrap_err(), ScriptError::Empty);
        let e = Script::parse("{\"id\":\"a.b\"}\nnot json\n").unwrap_err().to_string();
        assert!(e.starts_with("line 2:"), "{e}");
        let e = Script::parse("[{\"id\":\"a.b\"},{\"idd\":\"c.d\"}]").unwrap_err().to_string();
        assert!(e.starts_with("step 2:") && e.contains("idd"), "{e}");
        assert_eq!(Script::parse("42").unwrap_err(), ScriptError::WrongShape("number"));
        assert_eq!(Script::parse("\"x\"").unwrap_err(), ScriptError::WrongShape("string"));
        let e = Script::parse("{\"command\":\"a.b\"}").unwrap_err().to_string();
        assert!(e.contains("the step"), "{e}");
    }
}
