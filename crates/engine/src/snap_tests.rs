//! Tests of the shared `Snap` argument: one type for `arrow.move_point`, `element.move` and `element.resize`.

use serde_json::json;

use crate::snap::{Snap, SnapSwitches, SnapWord};
use crate::testing;

fn parse(v: serde_json::Value) -> Result<Snap, serde_json::Error> {
    serde_json::from_value(v)
}

#[test]
fn the_words_and_the_switches_parse_and_the_default_is_auto() {
    assert_eq!(parse(json!("auto")).unwrap(), Snap::Word(SnapWord::Auto));
    assert_eq!(parse(json!("none")).unwrap(), Snap::Word(SnapWord::None));
    assert_eq!(
        parse(json!({"grid": true, "angle": false})).unwrap(),
        Snap::Switches(SnapSwitches { grid: Some(true), smart: None, angle: Some(false) })
    );
    assert_eq!(Snap::default(), Snap::Word(SnapWord::Auto));
    assert!(parse(json!("sometimes")).is_err());
    assert!(parse(json!({"grid": true, "diagonal": true})).is_err(), "unknown switches are refused");
}

#[test]
fn resolving_follows_the_settings_unless_the_argument_says_otherwise() {
    let mut s = testing::headless_session();
    assert!(s.settings.set_raw("snap.grid", json!(true)));
    assert!(s.settings.set_raw("snap.angle", json!(true)));
    let auto = Snap::default().resolve(&s);
    assert_eq!((auto.grid, auto.smart, auto.angle), (true, false, true));
    let none = Snap::Word(SnapWord::None).resolve(&s);
    assert_eq!((none.grid, none.smart, none.angle), (false, false, false));
    let some = Snap::Switches(SnapSwitches { grid: Some(false), smart: Some(true), angle: None }).resolve(&s);
    assert_eq!((some.grid, some.smart, some.angle), (false, true, true), "an absent switch follows its setting");
}
