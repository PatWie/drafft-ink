//! The generated arguments form, drawn: combo box for an enum, required fields first, optional ones
//! behind "More".
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use drafftink_ui::args_form::{self, Field, FormValues};
use drafftink_ui::model;
use drafftink_ui::theme::{self, Palette};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

struct Form {
    schema: Value,
    fields: Vec<Field>,
    values: FormValues,
}

fn form_for(id: &str) -> Harness<'static, Form> {
    let schema = model::args_schema(id).expect("schema");
    let fields = args_form::fields(&schema);
    let values = args_form::initial_values(&fields, model::example_args(id).as_ref());
    let mut h = Harness::builder()
        .with_size(egui::vec2(600.0, 500.0))
        .with_context_setup(|ctx| {
            theme::install_fonts(ctx);
            theme::apply(ctx, None);
        })
        .build_ui_state(
            |ui, f: &mut Form| {
                args_form::show_fields(ui, &Palette::LIGHT, &f.fields, &mut f.values, None);
            },
            Form { schema, fields, values },
        );
    h.run();
    h
}

#[test]
fn an_enum_property_is_a_combo_box_prefilled_from_the_example() {
    let mut h = form_for("arrow.set_type");
    h.get_by_label("type");
    assert_eq!(h.state().values.get("type"), Some(&args_form::FieldValue::Choice(json!("elbow"))));
    h.get_by_role(Role::ComboBox).click();
    h.run();
    h.get_by_label("round").click();
    h.run();
    assert_eq!(args_form::build_args(&h.state().schema, &h.state().values), Ok(json!({ "type": "round" })));
}

#[test]
fn optional_properties_are_behind_more() {
    let mut h = form_for("arrow.set_type");
    assert!(h.query_by_label("ids").is_none(), "optional fields are hidden at first");
    h.get_by_label("More").click();
    h.run();
    h.get_by_label("ids");
    assert_eq!(
        args_form::build_args(&h.state().schema, &h.state().values),
        Ok(json!({ "type": "elbow" })),
        "an optional field nobody touched is not sent"
    );
}

#[test]
fn a_text_property_edits_the_value() {
    let mut h = form_for("tab.rename");
    h.get_by_label("name");
    let field = h.get_by_role(Role::TextInput);
    assert_eq!(field.value().as_deref(), Some("Architecture"));
    field.focus();
    h.run();
    h.get_by_role(Role::TextInput).type_text("2");
    h.run();
    assert_eq!(args_form::build_args(&h.state().schema, &h.state().values), Ok(json!({ "name": "Architecture2" })));
}

#[test]
fn an_optional_boolean_is_sent_only_after_it_is_switched_on() {
    let mut h = form_for("style.apply_theme");
    h.get_by_label("More").click();
    h.run();
    h.get_by_label("set_background");
    assert!(!h.state().values.contains_key("set_background"));
    // The "Set" check boxes: scope, set_background (ids is raw JSON and has none).
    let sets: Vec<_> = h.get_all_by_label("Set").collect();
    assert_eq!(sets.len(), 2, "one switch per optional control");
    sets.last().unwrap().click();
    h.run();
    assert_eq!(h.state().values.get("set_background"), Some(&args_form::FieldValue::Bool(false)));
    let built = args_form::build_args(&h.state().schema, &h.state().values).unwrap();
    assert_eq!(built.get("set_background"), Some(&json!(false)));
    assert_eq!(built.get("name"), Some(&json!("pastel")));
}
