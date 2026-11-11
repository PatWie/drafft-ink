//! Native builds of the web app are empty: the page is the wasm library (`src/lib.rs`), built by
//! `scripts/build-web.sh`. This file keeps `cargo check --workspace` and `cargo run` harmless.

fn main() {}
