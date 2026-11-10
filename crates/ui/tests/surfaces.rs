//! One test per surface: clicking a control makes the expected command call, a refused control is
//! greyed with the refusal text.

mod common;

use common::{Fixture, click, harness};
use drafftink_engine::Origin;
use serde_json::json;

#[test]
fn toolbar_click_runs_tool_set_with_the_tool() {
    let mut h = harness(Fixture::new());
    h.run();
    let calls = click(&mut h, "Rectangle tool");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].id, "tool.set");
    assert_eq!(calls[0].args, json!({"tool": "rectangle"}));
    assert_eq!(calls[0].origin, Origin::Ui);
}
