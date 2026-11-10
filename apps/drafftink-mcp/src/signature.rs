//! One-line argument signatures from JSON Schemas, for tool descriptions.
//!
//! A tool description lists every command with its arguments, for example
//! `kind: "rectangle"|"ellipse", bounds: Rect, label?: string`. The signature is cut from the command's
//! own schema, so it cannot drift from it. The full schema is one `describe` call away.

use serde_json::Value;

/// Longest signature in characters. Longer ones end in `...`.
const MAX_LEN: usize = 220;
/// Most enum values that are spelled out.
const MAX_ENUM: usize = 6;
/// Deepest nesting that is spelled out.
const MAX_DEPTH: usize = 2;

fn definition<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let name = reference.strip_prefix("#/$defs/").or_else(|| reference.strip_prefix("#/definitions/"))?;
    root.get("$defs").or_else(|| root.get("definitions"))?.get(name)
}

fn join(parts: Vec<String>, sep: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    for p in parts {
        if !seen.contains(&p) {
            seen.push(p);
        }
    }
    seen.join(sep)
}

/// A short type name for a schema node: `number`, `"a"|"b"`, `Rect`, `string[]`, `{x, y}`.
fn type_name(node: &Value, root: &Value, depth: usize) -> String {
    if let Some(r) = node.get("$ref").and_then(Value::as_str) {
        return match definition(root, r) {
            Some(def) if depth < MAX_DEPTH && def.get("properties").is_some() => type_name(def, root, depth + 1),
            Some(def) if def.get("enum").is_some() || def.get("oneOf").is_some() => type_name(def, root, depth + 1),
            _ => r.rsplit('/').next().unwrap_or("value").to_owned(),
        };
    }
    if let Some(values) = node.get("enum").and_then(Value::as_array) {
        let shown: Vec<String> = values.iter().take(MAX_ENUM).map(|v| v.to_string()).collect();
        let more = if values.len() > MAX_ENUM { "|..." } else { "" };
        return format!("{}{more}", join(shown, "|"));
    }
    if let Some(c) = node.get("const") {
        return c.to_string();
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(options) = node.get(key).and_then(Value::as_array) {
            let names: Vec<String> =
                options.iter().map(|o| type_name(o, root, depth)).filter(|n| n != "null").collect();
            let nullable = options.iter().any(|o| o.get("type").and_then(Value::as_str) == Some("null"));
            let text = join(names, "|");
            return if nullable { format!("{text}?") } else { text };
        }
    }
    match node.get("type") {
        Some(Value::Array(types)) => {
            let names: Vec<String> =
                types.iter().filter_map(Value::as_str).filter(|t| *t != "null").map(str::to_owned).collect();
            join(names, "|")
        }
        Some(Value::String(t)) if t == "array" => {
            let item = node.get("items").map_or_else(|| "value".to_owned(), |i| type_name(i, root, depth + 1));
            format!("{item}[]")
        }
        Some(Value::String(t)) if t == "object" && node.get("properties").is_some() && depth < MAX_DEPTH => {
            format!("{{{}}}", properties(node, root, depth + 1))
        }
        Some(Value::String(t)) => t.clone(),
        _ => "value".to_owned(),
    }
}

fn properties(node: &Value, root: &Value, depth: usize) -> String {
    let Some(props) = node.get("properties").and_then(Value::as_object) else { return String::new() };
    let required: Vec<&str> = node
        .get("required")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |r| r.iter().filter_map(Value::as_str).collect());
    // Required properties in the order the schema lists them (the struct's order), then the optional ones.
    let first = required.iter().filter_map(|name| props.get_key_value(*name));
    let rest = props.iter().filter(|(k, _)| !required.contains(&k.as_str()));
    first
        .chain(rest)
        .map(|(name, schema)| {
            let optional = if required.contains(&name.as_str()) { "" } else { "?" };
            format!("{name}{optional}: {}", type_name(schema, root, depth))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The signature of an argument schema; `no arguments` for an empty object.
pub fn signature(args_schema: &Value) -> String {
    let text = properties(args_schema, args_schema, 0);
    if text.is_empty() {
        return "no arguments".to_owned();
    }
    if text.chars().count() > MAX_LEN {
        let cut: String = text.chars().take(MAX_LEN).collect();
        return format!("{}...", cut.trim_end_matches([',', ' ']));
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn spells_out_required_first_and_marks_optional() {
        let s = json!({"type":"object","properties":{"b":{"type":"number"},"a":{"type":"string"},"c":{"type":["integer","null"]}},"required":["b"]});
        assert_eq!(signature(&s), "b: number, a?: string, c?: integer");
    }

    #[test]
    fn follows_references_enums_and_arrays() {
        let s = json!({
            "type":"object",
            "properties":{
                "kind":{"enum":["rectangle","ellipse"]},
                "bounds":{"$ref":"#/$defs/Rect"},
                "ids":{"type":"array","items":{"type":"string"}},
                "at":{"anyOf":[{"$ref":"#/$defs/Pt"},{"type":"null"}]}
            },
            "required":["kind","bounds"],
            "$defs":{"Rect":{"type":"object","properties":{"x":{"type":"number"},"w":{"type":"number"}},"required":["x","w"]},"Pt":{"type":"object"}}
        });
        assert_eq!(
            signature(&s),
            r#"kind: "rectangle"|"ellipse", bounds: {x: number, w: number}, at?: Pt?, ids?: string[]"#
        );
    }

    #[test]
    fn empty_and_hostile_schemas_do_not_panic() {
        assert_eq!(signature(&json!({"type":"object","properties":{}})), "no arguments");
        assert_eq!(signature(&json!(null)), "no arguments");
        assert_eq!(
            signature(&json!({"properties":{"a":{"$ref":"#/$defs/Loop"}},"$defs":{"Loop":{"$ref":"#/$defs/Loop"}}})),
            "a?: Loop"
        );
        let long = json!({"properties": (0..60).map(|i| (format!("field_{i}"), json!({"type":"string"}))).collect::<serde_json::Map<_,_>>()});
        assert!(signature(&long).ends_with("..."));
        assert!(signature(&long).chars().count() <= MAX_LEN + 3);
    }
}
