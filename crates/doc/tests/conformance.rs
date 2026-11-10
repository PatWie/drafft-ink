#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The scene crate's store-generic conformance suite, run against the Loro store.

use drafftink_doc::LoroStore;

#[test]
fn loro_store_passes_the_scene_conformance_suite() {
    if let Err(failures) = drafftink_scene::conformance::run(LoroStore::new) {
        panic!("{}", failures.join("\n"));
    }
}
