//! drafftink-testkit (L7): the headless test rig.
//!
//! * [`Harness`]: a [`drafftink_engine::Session`] over fake ports and the real typesetter. Methods only for what
//!   commands cannot do; each states its reason (see the module docs of [`harness`]).
//! * [`scenario`]: JSON scenario files, one generated `#[test]` per file (`tests/scenarios.rs`).
//! * [`scenegen`]: the deterministic scene generator (1k, 10k, 100k elements) shared with the perf benches.
//! * [`expect`] and [`path`]: the checks and the JSONPath-lite that scenarios use.
//!
//! Other crates use this one as a dev-dependency only (`cargo xtask layers`). `docs/testing.md` is the guide.

pub mod expect;
pub mod harness;
pub mod path;
pub mod scenario;

/// The deterministic scene generator. It lives in `drafftink-scene` (feature `scenegen`) so the apps can use it
/// without the test rig; this path keeps the tests and benches as they were.
pub use drafftink_scene::scenegen;
pub use harness::{GpuStatus, Harness, HarnessError, PixelArgs, ProbePoint, gpu_required, gpu_status};
pub use scenario::{Outcome, Scenario};
