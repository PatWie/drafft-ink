//! The JSON Schemas of the commands, and the table that stands in for them where they are left out of the build.
//!
//! Each command has an argument type and an output type with a derived schema. Generating them costs code: in the
//! web build it was 89 KB of the brotli-compressed module (2.8%). Without the cargo feature `schemas` the schema
//! code is not linked; the page fetches the same schemas as a file made at build time (`drafftink-cli schemas`),
//! hands it to [`install`], and every reader goes through [`CommandSpec::args_schema`] and
//! [`CommandSpec::output_schema`], which read the table. Without the feature the table does not exist and the
//! accessors generate the schema from the type, as before.
//!
//! Until the table is installed a reader gets an empty schema; [`available`] says whether that is the case, so a
//! reader that must not guess (`engine.describe`, the arguments forms) can wait or refuse in words.

#[cfg(not(feature = "schemas"))]
use std::sync::Arc;

#[cfg(not(feature = "schemas"))]
use crate::args::ArgInfo;
#[cfg(not(feature = "schemas"))]
use crate::spec::Schema;

#[cfg(not(feature = "schemas"))]
mod table {
    use std::collections::HashMap;
    use std::sync::{Arc, OnceLock};

    use serde_json::Value;

    use super::{Entry, SchemaError, empty};
    use crate::args::{ArgInfo, closed_object, schema_keys};
    use crate::spec::Schema;

    static TABLE: OnceLock<HashMap<String, Entry>> = OnceLock::new();
    static FAILURE: OnceLock<String> = OnceLock::new();

    fn schema_at(entry: &Value, key: &'static str) -> Result<Schema, SchemaError> {
        let value = entry.get(key).ok_or(SchemaError::Missing(key))?.clone();
        Schema::try_from(value).map_err(|_| SchemaError::NotASchema(key))
    }

    pub(super) fn install(text: &str) -> Result<usize, SchemaError> {
        let root: Value = serde_json::from_str(text).map_err(|e| SchemaError::Json(e.to_string()))?;
        let commands = root.get("commands").and_then(Value::as_object).ok_or(SchemaError::Missing("commands"))?;
        let mut table = HashMap::with_capacity(commands.len());
        for (id, entry) in commands {
            let args = schema_at(entry, "args")?;
            let output = schema_at(entry, "output")?;
            let info = Arc::new(ArgInfo { keys: schema_keys(&args), closed: closed_object(&args) });
            table.insert(id.clone(), Entry { args, output, info });
        }
        let count = table.len();
        // A second install (a reload of the table) keeps the first: the schemas of one build do not change.
        let _ = TABLE.set(table);
        Ok(count)
    }

    pub(super) fn get(id: &str) -> Option<&'static Entry> {
        TABLE.get()?.get(id)
    }

    pub(super) fn loaded() -> bool {
        TABLE.get().is_some()
    }

    pub(super) fn fail(reason: String) {
        let _ = FAILURE.set(reason);
    }

    pub(super) fn failure() -> Option<&'static str> {
        FAILURE.get().map(String::as_str)
    }

    pub(super) fn schema_or_empty(id: &str, pick: impl Fn(&Entry) -> &Schema) -> Schema {
        get(id).map_or_else(empty, |e| pick(e).clone())
    }
}

/// One command's schemas, parsed.
#[cfg(not(feature = "schemas"))]
#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) args: Schema,
    pub(crate) output: Schema,
    pub(crate) info: Arc<ArgInfo>,
}

/// Why a schema table was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    /// The text is not JSON.
    #[error("the schema table is not JSON: {0}")]
    Json(String),
    /// A key is missing.
    #[error("the schema table has no `{0}`")]
    Missing(&'static str),
    /// A value is not a JSON Schema.
    #[error("`{0}` in the schema table is not a JSON Schema")]
    NotASchema(&'static str),
}

/// The schema that stands in while there is none: it says nothing.
#[cfg(not(feature = "schemas"))]
pub(crate) fn empty() -> Schema {
    Schema::from(true)
}

/// Where the schemas of a command stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaState {
    /// Generated from the types (a build with `schemas`), or installed.
    Ready,
    /// Left out of this build and not installed yet.
    Loading,
    /// Left out of this build and the host could not load them.
    Failed(String),
}

/// Installs the table that [`dump`] wrote. Returns the number of commands. A second call keeps the first table.
/// Without the feature the schemas are generated and the table is not needed, so this only checks it.
pub fn install(text: &str) -> Result<usize, SchemaError> {
    #[cfg(not(feature = "schemas"))]
    {
        table::install(text)
    }
    #[cfg(feature = "schemas")]
    {
        let root: serde_json::Value = serde_json::from_str(text).map_err(|e| SchemaError::Json(e.to_string()))?;
        root.get("commands").and_then(serde_json::Value::as_object).map(serde_json::Map::len).ok_or(SchemaError::Missing("commands"))
    }
}

/// Records that the host could not load the table, so readers can say why they have no schemas.
pub fn fail(reason: impl Into<String>) {
    #[cfg(not(feature = "schemas"))]
    table::fail(reason.into());
    #[cfg(feature = "schemas")]
    let _ = reason.into();
}

/// Where the schemas stand.
pub fn state() -> SchemaState {
    #[cfg(not(feature = "schemas"))]
    {
        if table::loaded() {
            SchemaState::Ready
        } else {
            table::failure().map_or(SchemaState::Loading, |why| SchemaState::Failed(why.to_owned()))
        }
    }
    #[cfg(feature = "schemas")]
    {
        SchemaState::Ready
    }
}

/// True when the schemas can be read now.
pub fn available() -> bool {
    state() == SchemaState::Ready
}

/// The argument schema of command `id`, empty until the table is installed.
#[cfg(not(feature = "schemas"))]
pub(crate) fn args_of(id: &str) -> Schema {
    table::schema_or_empty(id, |e| &e.args)
}

/// The output schema of command `id`, empty until the table is installed.
#[cfg(not(feature = "schemas"))]
pub(crate) fn output_of(id: &str) -> Schema {
    table::schema_or_empty(id, |e| &e.output)
}

/// What the argument parser needs to know about command `id`: its keys, and whether it refuses others.
#[cfg(not(feature = "schemas"))]
pub(crate) fn arg_info_of(id: &str) -> Arc<ArgInfo> {
    table::get(id).map_or_else(|| Arc::new(ArgInfo { keys: Vec::new(), closed: false }), |e| e.info.clone())
}

/// The table for [`install`], written from the types: `{"commands": {id: {"args": ..., "output": ...}}}`. Only a
/// build that generates schemas can make it.
#[cfg(feature = "schemas")]
pub fn dump(registry: &crate::registry::Registry) -> serde_json::Value {
    let commands: serde_json::Map<String, serde_json::Value> = registry
        .all()
        .iter()
        .map(|spec| (spec.id.to_owned(), serde_json::json!({ "args": spec.args_schema(), "output": spec.output_schema() })))
        .collect();
    serde_json::json!({ "version": 1, "commands": commands })
}

#[cfg(all(test, feature = "schemas"))]
mod tests {
    use super::*;
    use crate::registry::Registry;

    #[test]
    fn the_dumped_table_has_every_command_and_installs() {
        let registry = Registry::global();
        let table = dump(&registry);
        let commands = table["commands"].as_object().unwrap();
        assert_eq!(commands.len(), registry.len());
        for spec in registry.all() {
            let entry = &commands[spec.id];
            assert_eq!(entry["args"], spec.args_schema().to_value(), "{}", spec.id);
            assert_eq!(entry["output"], spec.output_schema().to_value(), "{}", spec.id);
        }
        assert_eq!(install(&table.to_string()), Ok(registry.len()));
        assert!(available());
        assert_eq!(state(), SchemaState::Ready);
    }

    #[test]
    fn a_table_without_commands_is_refused_in_words() {
        assert_eq!(install("{}"), Err(SchemaError::Missing("commands")));
        assert!(matches!(install("nope"), Err(SchemaError::Json(_))));
    }
}
