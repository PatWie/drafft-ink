//! Argument parsing: JSON to a typed struct with `bad_args` refusals that name the key.
//!
//! * `null` or an absent object means `{}`.
//! * An unknown top-level key is named, with the closest valid key.
//! * A wrong type or value names the path of the key (`props.stroke_width`) and serde's reason.
//! * Helpers for the checks every handler needs: finite numbers within a limit, non-empty lists, lengths.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::refusal::{Refusal, closest, echo};
use crate::spec::Schema;

/// Largest coordinate or length a command accepts in world units. The geometry code tolerates far more
/// (1e150); this keeps the numbers in a range where later arithmetic stays finite.
pub const MAX_WORLD: f64 = 1.0e9;

/// The top-level property names of an argument schema. Empty for schemas without `properties`
/// (a struct with only flattened or untagged parts).
pub fn schema_keys(schema: &Schema) -> Vec<String> {
    let mut keys: Vec<String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// The names of the required top-level properties.
pub fn schema_required(schema: &Schema) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// True when the schema describes an object whose keys are all listed in `properties`.
pub(crate) fn closed_object(schema: &Schema) -> bool {
    schema.get("additionalProperties") == Some(&Value::Bool(false))
}

/// What the engine needs to know about an argument type, computed once per type.
#[derive(Debug, Clone)]
pub struct ArgInfo {
    /// Top-level property names, sorted. Empty when the schema lists none.
    pub keys: Vec<String>,
    /// The type refuses unknown keys (`deny_unknown_fields`).
    pub closed: bool,
}

/// The [`ArgInfo`] of `A`, from a per-type cache: building a schema is too slow to repeat per call.
pub fn arg_info<A: JsonSchema + 'static>() -> std::sync::Arc<ArgInfo> {
    use std::any::TypeId;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<TypeId, Arc<ArgInfo>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .entry(TypeId::of::<A>())
        .or_insert_with(|| {
            let schema = schemars::schema_for!(A);
            Arc::new(ArgInfo { keys: schema_keys(&schema), closed: closed_object(&schema) })
        })
        .clone()
}

/// Parses `args` as `A`. See the module docs for the refusal wording. The happy path builds no schema.
pub fn parse_args<A: DeserializeOwned + JsonSchema + 'static>(command: &str, args: &Value) -> Result<A, Refusal> {
    let empty = Value::Object(serde_json::Map::new());
    let args = if args.is_null() { &empty } else { args };
    if !args.is_object() {
        // A command whose arguments are not an object (rare) still gets a chance to deserialise.
        let probe: Result<A, _> = serde_path_to_error::deserialize(args);
        return probe.map_err(|e| {
            Refusal::bad_args(command, format!("the arguments must be a JSON object ({})", echo(&e.inner().to_string())))
        });
    }
    match serde_path_to_error::deserialize::<_, A>(args) {
        Ok(a) => Ok(a),
        Err(e) => {
            let path = e.path().to_string();
            let reason = e.inner().to_string();
            let info = arg_info::<A>();
            if info.closed
                && !info.keys.is_empty()
                && let Some(bad) = args.as_object().and_then(|o| o.keys().find(|k| !info.keys.iter().any(|v| v == *k)))
            {
                return Err(unknown_key(command, bad, &info.keys));
            }
            if let Some(bad) = unknown_field_in(&reason) {
                return Err(unknown_key(command, &bad, &info.keys));
            }
            if path == "." || path.is_empty() {
                Err(Refusal::bad_args(command, echo_reason(&reason)))
            } else {
                Err(Refusal::bad_arg(command, &path, echo_reason(&reason)))
            }
        }
    }
}

fn echo_reason(reason: &str) -> String {
    let mut it = reason.chars();
    let head: String = it.by_ref().take(200).collect();
    if it.next().is_some() { format!("{head}…") } else { head }
}

/// The key named by serde's "unknown field `x`, expected ..." message.
fn unknown_field_in(reason: &str) -> Option<String> {
    let rest = reason.strip_prefix("unknown field `")?;
    let end = rest.find('`')?;
    rest.get(..end).map(str::to_owned)
}

fn unknown_key(command: &str, bad: &str, valid: &[String]) -> Refusal {
    let near = closest(bad, valid.iter().map(String::as_str), 1);
    let mut detail = format!("unknown key `{}`", echo(bad));
    if let Some(n) = near.first() {
        detail.push_str(&format!(", did you mean `{n}`?"));
    } else if !valid.is_empty() && valid.len() <= 12 {
        let list: Vec<String> = valid.iter().map(|k| format!("`{k}`")).collect();
        detail.push_str(&format!(". Valid keys: {}", list.join(", ")));
    }
    Refusal::bad_args(command, detail).with_details(serde_json::json!({ "key": echo(bad), "did_you_mean": near.first() }))
}

/// Refuses an unknown top-level key (with the closest valid one). Does nothing when `keys` is empty.
pub(crate) fn check_top_keys(command: &str, args: &Value, keys: &[String]) -> Result<(), Refusal> {
    if keys.is_empty() {
        return Ok(());
    }
    if let Some(obj) = args.as_object()
        && let Some(bad) = obj.keys().find(|k| !keys.iter().any(|v| v == *k))
    {
        return Err(unknown_key(command, bad, keys));
    }
    Ok(())
}

/// A finite number within `±MAX_WORLD`, else `bad_args` naming `key`.
pub fn world(command: &str, key: &str, v: f64) -> Result<f64, Refusal> {
    if v.is_finite() && v.abs() <= MAX_WORLD {
        Ok(v)
    } else {
        Err(Refusal::bad_arg(command, key, format!("must be a finite number between -{MAX_WORLD:e} and {MAX_WORLD:e}")))
    }
}

/// A finite number in `[min, max]`, else `bad_args` naming `key`.
pub fn in_range(command: &str, key: &str, v: f64, min: f64, max: f64) -> Result<f64, Refusal> {
    if v.is_finite() && v >= min && v <= max {
        Ok(v)
    } else {
        Err(Refusal::bad_arg(command, key, format!("must be a number from {min} to {max}")))
    }
}

/// A finite number above zero and at most `max`.
pub fn positive(command: &str, key: &str, v: f64, max: f64) -> Result<f64, Refusal> {
    if v.is_finite() && v > 0.0 && v <= max {
        Ok(v)
    } else {
        Err(Refusal::bad_arg(command, key, format!("must be a number above 0 and at most {max}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::NoArgs;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Two {
        scroll: Option<[f64; 2]>,
        zoom: Option<f64>,
    }

    #[test]
    fn unknown_key_is_named_with_the_closest_valid_key() {
        let e = parse_args::<Two>("view.set", &json!({"zom": 2})).unwrap_err();
        assert_eq!(e.code, "bad_args");
        assert!(e.message.contains("unknown key `zom`") && e.message.contains("did you mean `zoom`"), "{}", e.message);
    }

    #[test]
    fn wrong_type_names_the_key() {
        let e = parse_args::<Two>("view.set", &json!({"zoom": "big"})).unwrap_err();
        assert!(e.message.contains("`zoom`"), "{}", e.message);
        assert!(e.message.contains("See `engine.describe`"));
    }

    #[test]
    fn null_means_empty_and_scalars_are_refused() {
        assert_eq!(parse_args::<Two>("x.y", &Value::Null).unwrap(), Two { scroll: None, zoom: None });
        assert!(parse_args::<NoArgs>("x.y", &json!(5)).is_err());
        assert!(parse_args::<NoArgs>("x.y", &json!({"a": 1})).is_err());
    }

    #[test]
    fn helpers_reject_non_finite_and_huge() {
        assert!(world("a.b", "dx", 5.0).is_ok());
        assert!(world("a.b", "dx", f64::NAN).is_err());
        assert!(world("a.b", "dx", 1e12).is_err());
        assert!(positive("a.b", "zoom", 0.0, 10.0).is_err());
        assert!(in_range("a.b", "f", 3.0, 1.0, 2.0).is_err());
    }
}
