//! A small form generated from a command's JSON Schema.
//!
//! The palette opens it for a command that cannot run with `{}` (its schema has required properties and
//! no titled binding fixes them). The schema comes from [`crate::model::args_schema`] and is the one
//! `schemars` 1.x writes for the argument struct of the command. The form understands:
//!
//! * `$ref` into `$defs` (a sibling `description` of the reference wins over the target's);
//! * `oneOf` / `anyOf` / `enum` of constants: a combo box;
//! * `boolean`: a check box; `integer` and `number`: a drag value; `string`: a text field;
//! * `Option<T>`, written as `anyOf [T, null]` or `type: [T, "null"]`: unwrapped to `T`;
//! * anything else (arrays, objects, mixed unions): a text field that holds raw JSON, parsed with
//!   `serde_json` and checked while the user types.
//!
//! The required properties come first. The optional ones are behind a collapsing "More" section and
//! are left out of the arguments until the user sets them, so the command's own defaults apply.
//!
//! [`build_args`] is a pure function from schema and values to arguments and is unit-tested. The
//! drawing code only edits a [`FormValues`].

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use egui::{ComboBox, DragValue, Id, Key, RichText, TextEdit, Ui};
use serde_json::{Map, Number, Value};

use crate::theme::{Palette, TextSize};

/// How deep `$ref` and `anyOf` are followed before the form gives up and shows raw JSON.
const MAX_DEPTH: u8 = 8;

/// The value of one field in the form.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// A check box.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A number.
    Number(f64),
    /// A text field. For a raw-JSON field this is the JSON text.
    Text(String),
    /// One of the constants of a combo box.
    Choice(Value),
}

/// The values of a form by property name. A property without an entry is left out of the arguments.
pub type FormValues = BTreeMap<String, FieldValue>;

/// One entry of a combo box.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// The constant sent to the command.
    pub value: Value,
    /// What the combo box shows.
    pub label: String,
    /// The schema's description of this entry, shown as a tooltip.
    pub doc: Option<String>,
}

/// Which control a property gets.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    /// A combo box over constants.
    Choice(Vec<Choice>),
    /// A check box.
    Bool,
    /// A drag value over integers, with the schema's bounds.
    Integer {
        /// `minimum`.
        min: Option<i64>,
        /// `maximum`.
        max: Option<i64>,
    },
    /// A drag value over numbers.
    Number,
    /// A text field.
    Text,
    /// A text field with JSON in it.
    Raw,
}

/// One property of the argument struct.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The property name.
    pub name: String,
    /// The control.
    pub kind: FieldKind,
    /// Listed in `required`.
    pub required: bool,
    /// The description from the schema.
    pub doc: String,
    /// The default from the schema, when it is not `null`.
    pub default: Option<Value>,
}

/// Follows `$ref` (a JSON pointer into the same document) to its target. A cycle or a missing target
/// stops at the last node that was found.
fn deref<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    let mut cur = node;
    for _ in 0..MAX_DEPTH {
        let target =
            cur.get("$ref").and_then(Value::as_str).and_then(|r| r.strip_prefix('#')).and_then(|p| root.pointer(p));
        match target {
            Some(t) => cur = t,
            None => break,
        }
    }
    cur
}

fn is_null_schema(node: &Value) -> bool {
    node.get("type").and_then(Value::as_str) == Some("null")
}

/// The single non-null type name of a `type` keyword, if there is exactly one.
fn type_name(node: &Value) -> Option<&str> {
    match node.get("type")? {
        Value::String(s) => Some(s.as_str()),
        Value::Array(a) => {
            let mut names = a.iter().filter_map(Value::as_str).filter(|n| *n != "null");
            let first = names.next()?;
            names.next().is_none().then_some(first)
        }
        _ => None,
    }
}

fn label_of(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_owned)
}

fn doc_of(root: &Value, node: &Value) -> Option<String> {
    node.get("description")
        .or_else(|| deref(root, node).get("description"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_owned())
}

/// A combo box over the alternatives when every one of them is a constant.
fn choices_of(root: &Value, alternatives: &[&Value]) -> Option<Vec<Choice>> {
    let mut out = Vec::with_capacity(alternatives.len());
    for alt in alternatives {
        let node = deref(root, alt);
        let value = node.get("const")?;
        out.push(Choice { value: value.clone(), label: label_of(value), doc: doc_of(root, alt) });
    }
    (!out.is_empty()).then_some(out)
}

fn kind_of(root: &Value, node: &Value, depth: u8) -> FieldKind {
    if depth > MAX_DEPTH {
        return FieldKind::Raw;
    }
    let node = deref(root, node);
    for keyword in ["oneOf", "anyOf"] {
        if let Some(alternatives) = node.get(keyword).and_then(Value::as_array) {
            let non_null: Vec<&Value> = alternatives.iter().filter(|a| !is_null_schema(a)).collect();
            return match non_null.as_slice() {
                [one] => kind_of(root, one, depth + 1),
                many => choices_of(root, many).map_or(FieldKind::Raw, FieldKind::Choice),
            };
        }
    }
    if let Some(values) = node.get("enum").and_then(Value::as_array) {
        let choices: Vec<Choice> =
            values.iter().map(|v| Choice { value: v.clone(), label: label_of(v), doc: None }).collect();
        return if choices.is_empty() { FieldKind::Raw } else { FieldKind::Choice(choices) };
    }
    if let Some(value) = node.get("const") {
        return FieldKind::Choice(vec![Choice { value: value.clone(), label: label_of(value), doc: None }]);
    }
    match type_name(node) {
        Some("boolean") => FieldKind::Bool,
        Some("integer") => FieldKind::Integer {
            min: node.get("minimum").and_then(Value::as_i64),
            max: node.get("maximum").and_then(Value::as_i64),
        },
        Some("number") => FieldKind::Number,
        Some("string") => FieldKind::Text,
        _ => FieldKind::Raw,
    }
}

/// The properties of an argument schema, required ones first and each group in schema order. A schema
/// without `properties` gives no fields.
pub fn fields(schema: &Value) -> Vec<Field> {
    let node = deref(schema, schema);
    let Some(props) = node.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let required: HashSet<&str> =
        node.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let mut out: Vec<Field> = props
        .iter()
        .map(|(name, prop)| Field {
            name: name.clone(),
            kind: kind_of(schema, prop, 0),
            required: required.contains(name.as_str()),
            doc: doc_of(schema, prop).unwrap_or_default(),
            default: prop.get("default").filter(|d| !d.is_null()).cloned(),
        })
        .collect();
    out.sort_by_key(|f| !f.required);
    out
}

/// True when the command has required properties, so it cannot run with `{}`. Computed once for the
/// whole registry.
///
/// Until the schemas are there (the web build fetches them after the first frame) no command is known to need
/// arguments, and the answer is not kept: the next call asks again.
pub fn requires_args(id: &str) -> bool {
    static REQUIRING: OnceLock<HashSet<&'static str>> = OnceLock::new();
    if let Some(known) = REQUIRING.get() {
        return known.contains(id);
    }
    if !drafftink_engine::schemas::available() {
        return false;
    }
    REQUIRING
        .get_or_init(|| {
            drafftink_engine::Registry::global()
                .all()
                .iter()
                .filter(|spec| {
                    serde_json::to_value(spec.args_schema())
                        .is_ok_and(|schema| fields(&schema).iter().any(|f| f.required))
                })
                .map(|spec| spec.id)
                .collect()
        })
        .contains(id)
}

/// The value a field starts with when nothing else is known: its default, else a neutral value.
pub fn blank(field: &Field) -> FieldValue {
    let default = field.default.as_ref();
    match &field.kind {
        FieldKind::Choice(choices) => {
            let pick = default.and_then(|d| choices.iter().find(|c| &c.value == d)).or_else(|| choices.first());
            FieldValue::Choice(pick.map_or(Value::Null, |c| c.value.clone()))
        }
        FieldKind::Bool => FieldValue::Bool(default.and_then(Value::as_bool).unwrap_or(false)),
        FieldKind::Integer { min, max } => {
            let n = default.and_then(Value::as_i64).unwrap_or(0);
            FieldValue::Int(n.max(min.unwrap_or(i64::MIN)).min(max.unwrap_or(i64::MAX)))
        }
        FieldKind::Number => FieldValue::Number(default.and_then(Value::as_f64).unwrap_or(0.0)),
        FieldKind::Text => FieldValue::Text(default.and_then(Value::as_str).unwrap_or_default().to_owned()),
        FieldKind::Raw => FieldValue::Text(String::new()),
    }
}

/// A JSON value as the value of a field of this kind, `None` when it does not fit.
fn value_from_json(kind: &FieldKind, v: &Value) -> Option<FieldValue> {
    match kind {
        FieldKind::Choice(choices) => choices.iter().any(|c| &c.value == v).then(|| FieldValue::Choice(v.clone())),
        FieldKind::Bool => v.as_bool().map(FieldValue::Bool),
        FieldKind::Integer { .. } => v.as_i64().map(FieldValue::Int),
        FieldKind::Number => v.as_f64().map(FieldValue::Number),
        FieldKind::Text => v.as_str().map(|s| FieldValue::Text(s.to_owned())),
        FieldKind::Raw => Some(FieldValue::Text(v.to_string())),
    }
}

/// The values a form starts with: every required field, taken from the worked example of the command
/// when it fits and from [`blank`] otherwise. Optional fields start unset, so the command's own
/// defaults apply and an example's placeholder ids are never sent by accident.
pub fn initial_values(fields: &[Field], example: Option<&Value>) -> FormValues {
    fields
        .iter()
        .filter(|f| f.required)
        .map(|f| {
            let from_example = example.and_then(|e| e.get(&f.name)).and_then(|v| value_from_json(&f.kind, v));
            (f.name.clone(), from_example.unwrap_or_else(|| blank(f)))
        })
        .collect()
}

fn mismatch(name: &str) -> String {
    format!("`{name}` holds a value of the wrong kind.")
}

/// Turns the values of a form into the arguments object. A required field without a value, a raw field
/// with text that is not JSON, an integer outside its bounds and a number that is not finite are
/// errors with a plain message. An optional field without a value (or with empty text) is left out.
pub fn build_args(schema: &Value, values: &FormValues) -> Result<Value, String> {
    let mut out = Map::new();
    for field in fields(schema) {
        let name = field.name.as_str();
        let Some(value) = values.get(name) else {
            if field.required {
                return Err(format!("`{name}` needs a value."));
            }
            continue;
        };
        let json = match (&field.kind, value) {
            (FieldKind::Choice(_), FieldValue::Choice(v)) => v.clone(),
            (FieldKind::Bool, FieldValue::Bool(b)) => Value::Bool(*b),
            (FieldKind::Integer { min, max }, FieldValue::Int(n)) => {
                if min.is_some_and(|m| *n < m) || max.is_some_and(|m| *n > m) {
                    return Err(format!("`{name}` must be {}.", bounds_text(*min, *max)));
                }
                Value::from(*n)
            }
            (FieldKind::Number, FieldValue::Number(x)) => match Number::from_f64(*x) {
                Some(n) => Value::Number(n),
                None => return Err(format!("`{name}` must be a finite number.")),
            },
            (FieldKind::Text, FieldValue::Text(s)) => {
                if s.is_empty() && !field.required {
                    continue;
                }
                Value::String(s.clone())
            }
            (FieldKind::Raw, FieldValue::Text(s)) => {
                if s.trim().is_empty() {
                    if field.required {
                        return Err(format!("`{name}` needs a value (JSON)."));
                    }
                    continue;
                }
                serde_json::from_str(s).map_err(|e| format!("`{name}` is not valid JSON: {e}."))?
            }
            _ => return Err(mismatch(name)),
        };
        out.insert(field.name.clone(), json);
    }
    Ok(Value::Object(out))
}

fn bounds_text(min: Option<i64>, max: Option<i64>) -> String {
    match (min, max) {
        (Some(lo), Some(hi)) => format!("between {lo} and {hi}"),
        (Some(lo), None) => format!("at least {lo}"),
        (None, Some(hi)) => format!("at most {hi}"),
        (None, None) => "a whole number".to_owned(),
    }
}

// ---- drawing ----

/// Draws the fields into `ui`: the required ones, then a collapsing "More" section with the optional
/// ones. `example` gives the hint text of raw fields. Returns true when the user pressed Enter in a
/// text field.
pub fn show_fields(
    ui: &mut Ui,
    pal: &Palette,
    fields: &[Field],
    values: &mut FormValues,
    example: Option<&Value>,
) -> bool {
    let mut submit = false;
    for field in fields.iter().filter(|f| f.required) {
        submit |= show_field(ui, pal, field, values, example);
    }
    if fields.iter().any(|f| !f.required) {
        egui::CollapsingHeader::new(RichText::new("More").font(TextSize::Label.hand()).color(pal.muted))
            .id_salt("args_form.more")
            .default_open(false)
            .show(ui, |ui| {
                for field in fields.iter().filter(|f| !f.required) {
                    submit |= show_field(ui, pal, field, values, example);
                }
            });
    }
    submit
}

fn show_field(ui: &mut Ui, pal: &Palette, field: &Field, values: &mut FormValues, example: Option<&Value>) -> bool {
    let mut submit = false;
    ui.add_space(4.0);
    ui.vertical(|ui| {
        let name = ui.label(RichText::new(&field.name).font(TextSize::Label.hand()));
        if !field.doc.is_empty() {
            ui.label(RichText::new(&field.doc).size(TextSize::Caption.points()).color(pal.muted));
        }
        // An optional control that has no empty state (a check box, a number, a combo box) is set
        // explicitly: the box in front of it adds or removes the property.
        let switchable = !field.required && !matches!(field.kind, FieldKind::Text | FieldKind::Raw);
        let mut enabled = true;
        if switchable {
            let mut on = values.contains_key(&field.name);
            if ui.checkbox(&mut on, "Set").changed() {
                if on {
                    values.insert(field.name.clone(), blank(field));
                } else {
                    values.remove(&field.name);
                }
            }
            enabled = on;
        }
        ui.add_enabled_ui(enabled, |ui| {
            let mut shown = values.get(&field.name).cloned().unwrap_or_else(|| blank(field));
            let hint = example.and_then(|e| e.get(&field.name)).map(Value::to_string);
            let (changed, entered) = edit_field(ui, pal, field, name.id, &mut shown, hint.as_deref());
            submit |= entered;
            // A disabled control cannot change, so `changed` implies the property is set. A field
            // without a switch is always set, so it is recorded on first sight.
            if changed || (!switchable && !values.contains_key(&field.name)) {
                values.insert(field.name.clone(), shown);
            }
        });
    });
    submit
}

/// Draws one control. Returns `(changed, pressed Enter)`.
fn edit_field(
    ui: &mut Ui,
    pal: &Palette,
    field: &Field,
    name: Id,
    value: &mut FieldValue,
    hint: Option<&str>,
) -> (bool, bool) {
    let mut changed = false;
    let mut entered = false;
    match (&field.kind, &mut *value) {
        (FieldKind::Choice(choices), FieldValue::Choice(current)) => {
            let shown =
                choices.iter().find(|c| &c.value == current).map_or_else(|| label_of(current), |c| c.label.clone());
            let combo =
                ComboBox::from_id_salt(("args_form.choice", &field.name)).selected_text(shown).show_ui(ui, |ui| {
                    for choice in choices {
                        let mut resp = ui.selectable_label(&choice.value == current, &choice.label);
                        if let Some(doc) = &choice.doc {
                            resp = resp.on_hover_text(doc);
                        }
                        if resp.clicked() {
                            *current = choice.value.clone();
                            changed = true;
                        }
                    }
                });
            combo.response.labelled_by(name);
        }
        (FieldKind::Bool, FieldValue::Bool(b)) => changed = ui.checkbox(b, "").labelled_by(name).changed(),
        (FieldKind::Integer { min, max }, FieldValue::Int(n)) => {
            let range = min.unwrap_or(i64::MIN)..=max.unwrap_or(i64::MAX);
            changed = ui.add(DragValue::new(n).range(range)).labelled_by(name).changed();
        }
        (FieldKind::Number, FieldValue::Number(x)) => {
            changed = ui.add(DragValue::new(x).speed(0.1)).labelled_by(name).changed()
        }
        (FieldKind::Text, FieldValue::Text(s)) => {
            let resp = ui
                .add(TextEdit::singleline(s).id_salt(("args_form.text", &field.name)).desired_width(f32::INFINITY))
                .labelled_by(name);
            changed = resp.changed();
            entered = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        }
        (FieldKind::Raw, FieldValue::Text(s)) => {
            let hint = hint.map_or_else(|| "JSON".to_owned(), |h| format!("JSON, for example {h}"));
            let resp = ui
                .add(
                    TextEdit::singleline(s)
                        .id_salt(("args_form.raw", &field.name))
                        .desired_width(f32::INFINITY)
                        .hint_text(hint),
                )
                .labelled_by(name);
            changed = resp.changed();
            entered = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if !s.trim().is_empty()
                && let Err(e) = serde_json::from_str::<Value>(s)
            {
                ui.label(
                    RichText::new(format!("Not valid JSON: {e}")).size(TextSize::Caption.points()).color(pal.danger),
                );
            }
        }
        _ => {
            // The value does not fit the control (a schema changed under an open form): start over.
            *value = blank(field);
            changed = true;
        }
    }
    (changed, entered)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Value {
        json!({
            "$defs": {
                "Kind": { "description": "A kind.", "oneOf": [
                    { "const": "sharp", "description": "Straight.", "type": "string" },
                    { "const": "round", "type": "string" },
                ] },
            },
            "type": "object",
            "properties": {
                "kind": { "$ref": "#/$defs/Kind", "description": "Which kind." },
                "on": { "type": ["boolean", "null"], "default": null },
                "count": { "type": "integer", "minimum": 0, "maximum": 9, "default": 1 },
                "gap": { "type": ["number", "null"], "default": null },
                "name": { "type": "string" },
                "note": { "anyOf": [{ "type": "string" }, { "type": "null" }], "default": null },
                "ids": { "type": ["array", "null"], "items": { "type": "string" }, "default": null },
                "tab": { "anyOf": [{ "type": "integer" }, { "type": "string" }] },
            },
            "required": ["kind", "name", "tab"],
        })
    }

    fn values(pairs: &[(&str, FieldValue)]) -> FormValues {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
    }

    fn field<'a>(all: &'a [Field], name: &str) -> &'a Field {
        all.iter().find(|f| f.name == name).expect("field")
    }

    #[test]
    fn fields_follow_refs_unwrap_options_and_put_required_first() {
        let all = fields(&schema());
        assert!(all.iter().take(3).all(|f| f.required), "{all:?}");
        assert!(all.iter().skip(3).all(|f| !f.required));
        let FieldKind::Choice(choices) = &field(&all, "kind").kind else {
            panic!("kind is a combo box");
        };
        assert_eq!(choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["sharp", "round"]);
        assert_eq!(choices[0].doc.as_deref(), Some("Straight."));
        assert_eq!(field(&all, "kind").doc, "Which kind.", "the description next to $ref wins");
        assert_eq!(field(&all, "on").kind, FieldKind::Bool);
        assert_eq!(field(&all, "count").kind, FieldKind::Integer { min: Some(0), max: Some(9) });
        assert_eq!(field(&all, "gap").kind, FieldKind::Number);
        assert_eq!(field(&all, "name").kind, FieldKind::Text);
        assert_eq!(field(&all, "note").kind, FieldKind::Text, "anyOf [string, null] unwraps");
        assert_eq!(field(&all, "ids").kind, FieldKind::Raw, "an array is raw JSON");
        assert_eq!(field(&all, "tab").kind, FieldKind::Raw, "integer or string is raw JSON");
        assert_eq!(field(&all, "count").default, Some(json!(1)));
        assert_eq!(field(&all, "on").default, None, "a null default is no default");
    }

    #[test]
    fn schema_without_properties_has_no_fields() {
        assert!(fields(&json!({ "type": "object" })).is_empty());
        assert_eq!(build_args(&json!({ "type": "object" }), &FormValues::new()), Ok(json!({})));
    }

    #[test]
    fn build_args_enum_bool_number_string_and_optional() {
        let s = schema();
        let v = values(&[
            ("kind", FieldValue::Choice(json!("round"))),
            ("name", FieldValue::Text("Login".into())),
            ("tab", FieldValue::Text("\"t2\"".into())),
            ("on", FieldValue::Bool(true)),
            ("count", FieldValue::Int(4)),
            ("gap", FieldValue::Number(2.5)),
            ("note", FieldValue::Text(String::new())),
            ("ids", FieldValue::Text("  ".into())),
        ]);
        assert_eq!(
            build_args(&s, &v),
            Ok(json!({ "kind": "round", "name": "Login", "tab": "t2", "on": true, "count": 4, "gap": 2.5 }))
        );
    }

    #[test]
    fn build_args_leaves_unset_optional_fields_out() {
        let v = values(&[
            ("kind", FieldValue::Choice(json!("sharp"))),
            ("name", FieldValue::Text(String::new())),
            ("tab", FieldValue::Text("1".into())),
        ]);
        assert_eq!(build_args(&schema(), &v), Ok(json!({ "kind": "sharp", "name": "", "tab": 1 })));
    }

    #[test]
    fn build_args_raw_json_parses_arrays_and_objects() {
        let mut v = values(&[
            ("kind", FieldValue::Choice(json!("sharp"))),
            ("name", FieldValue::Text("x".into())),
            ("tab", FieldValue::Text("0".into())),
        ]);
        v.insert("ids".into(), FieldValue::Text(r#"["a", "b"]"#.into()));
        assert_eq!(build_args(&schema(), &v), Ok(json!({ "kind": "sharp", "name": "x", "tab": 0, "ids": ["a", "b"] })));
    }

    #[test]
    fn build_args_reports_invalid_json_inline() {
        let mut v = values(&[
            ("kind", FieldValue::Choice(json!("sharp"))),
            ("name", FieldValue::Text("x".into())),
            ("tab", FieldValue::Text("t2".into())),
        ]);
        let err = build_args(&schema(), &v).expect_err("t2 is not JSON");
        assert!(err.starts_with("`tab` is not valid JSON"), "{err}");
        v.insert("tab".into(), FieldValue::Text(String::new()));
        let err = build_args(&schema(), &v).expect_err("empty required raw field");
        assert!(err.contains("`tab` needs a value"), "{err}");
    }

    #[test]
    fn build_args_rejects_missing_required_out_of_range_and_non_finite() {
        let mut v = values(&[("kind", FieldValue::Choice(json!("sharp"))), ("tab", FieldValue::Text("0".into()))]);
        let err = build_args(&schema(), &v).expect_err("name is missing");
        assert_eq!(err, "`name` needs a value.");
        v.insert("name".into(), FieldValue::Text("x".into()));
        v.insert("count".into(), FieldValue::Int(10));
        assert_eq!(build_args(&schema(), &v), Err("`count` must be between 0 and 9.".to_owned()));
        v.insert("count".into(), FieldValue::Int(3));
        v.insert("gap".into(), FieldValue::Number(f64::NAN));
        assert_eq!(build_args(&schema(), &v), Err("`gap` must be a finite number.".to_owned()));
        v.insert("gap".into(), FieldValue::Bool(true));
        assert_eq!(build_args(&schema(), &v), Err("`gap` holds a value of the wrong kind.".to_owned()));
    }

    #[test]
    fn initial_values_take_required_fields_from_the_example_and_skip_optional_ones() {
        let all = fields(&schema());
        let example = json!({ "kind": "round", "name": "Login", "tab": "t2", "ids": ["a"], "count": 7 });
        let v = initial_values(&all, Some(&example));
        assert_eq!(v.get("kind"), Some(&FieldValue::Choice(json!("round"))));
        assert_eq!(v.get("name"), Some(&FieldValue::Text("Login".into())));
        assert_eq!(v.get("tab"), Some(&FieldValue::Text("\"t2\"".into())));
        assert!(!v.contains_key("ids") && !v.contains_key("count"), "{v:?}");
        let blank_values = initial_values(&all, None);
        assert_eq!(blank_values.get("kind"), Some(&FieldValue::Choice(json!("sharp"))), "the first constant");
        assert_eq!(blank_values.get("name"), Some(&FieldValue::Text(String::new())));
    }

    #[test]
    fn an_example_value_that_does_not_fit_falls_back_to_the_blank() {
        let all = fields(&schema());
        let v = initial_values(&all, Some(&json!({ "kind": "zigzag", "name": 5 })));
        assert_eq!(v.get("kind"), Some(&FieldValue::Choice(json!("sharp"))));
        assert_eq!(v.get("name"), Some(&FieldValue::Text(String::new())));
    }

    #[test]
    fn requires_args_matches_the_schemas_of_the_registry() {
        assert!(requires_args("arrow.set_type"));
        assert!(requires_args("tab.rename"));
        assert!(!requires_args("history.undo"));
        assert!(!requires_args("no.such.command"));
        for spec in drafftink_engine::Registry::global().all() {
            let schema = serde_json::to_value((spec.args)()).expect("schema");
            assert_eq!(requires_args(spec.id), fields(&schema).iter().any(|f| f.required), "{}", spec.id);
        }
    }

    #[test]
    fn every_registered_schema_builds_args_from_its_initial_values() {
        for spec in drafftink_engine::Registry::global().all() {
            let schema = serde_json::to_value((spec.args)()).expect("schema");
            let all = fields(&schema);
            let example =
                (!spec.example.is_empty()).then(|| serde_json::from_str::<Value>(spec.example).ok()).flatten();
            let v = initial_values(&all, example.as_ref());
            let built = build_args(&schema, &v);
            // A required raw field starts empty, which is an error the form shows inline; nothing else is.
            if let Err(e) = built {
                assert!(
                    all.iter().any(|f| f.required && f.kind == FieldKind::Raw),
                    "{}: unexpected error {e}",
                    spec.id
                );
            }
        }
    }
}
