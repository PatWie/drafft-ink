//! The web app: the wasm entry point `start`, the `window.drafftink` JS API and the agent relay client.
//!
//! On the web the whole app is `drafftink-app` over the platform in `drafftink_app::web`; this crate adds what
//! belongs to the page: the entry point, the scripting API and pairing with `drafftink-mcp --relay`. Native
//! builds contain only [`pairing`], the pure rules, so `cargo check --workspace` and the unit tests run
//! everywhere. `scripts/build-web.sh` builds the wasm and `docs/web-app.md` describes the result.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod pairing;

// The pure rules of the web platform live in `crates/app/src/web`, which only exists on wasm32. Compiling the
// same files here runs their tests on the build machine (the repository has no wasm test runner).
#[cfg(test)]
#[path = "../../../crates/app/src/web/cache.rs"]
#[allow(dead_code)]
mod platform_cache;
#[cfg(test)]
#[path = "../../../crates/app/src/web/pure.rs"]
#[allow(dead_code)]
mod platform_pure;

#[cfg(target_arch = "wasm32")]
mod agent;
#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod entry;
#[cfg(target_arch = "wasm32")]
mod js;
#[cfg(target_arch = "wasm32")]
mod scenario;
