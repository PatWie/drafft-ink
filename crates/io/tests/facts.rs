#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::field_reassign_with_default)]
//! The legacy fixtures against the facts the old code wrote (`cargo xtask fixtures --check` runs the same).

use std::path::Path;

use drafftink_io::facts::check_fixture_facts;

#[test]
fn every_legacy_fixture_matches_its_facts() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let report = check_fixture_facts(&dir);
    println!("{}", report.render());
    assert!(report.is_ok(), "{}", report.render());
    assert!(report.fixtures.len() >= 20, "{}", report.render());
}
