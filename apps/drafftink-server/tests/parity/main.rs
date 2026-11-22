#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Real sockets, real processes and the real clock.
#![allow(clippy::disallowed_methods)]
//! Transport parity (docs/design/collab-e2e.md section 15): one list of scenarios, run through the real native clients
//! (`SealedClient` over `NativeTransport` and `NativeBlobTransport`) against every relay we ship:
//!
//! | target | what runs | when |
//! |---|---|---|
//! | `in_process` | `drafftink_server::start` in this process, filesystem store | always |
//! | `child` | the real `drafftink-headless serve` process, filesystem store | always |
//! | `wrangler` | the demo Worker (TypeScript Worker, Durable Object, Rust wasm core) under `wrangler dev` | `DRAFFTINK_PARITY_WRANGLER=1` |
//! | `private` | the private Worker under `wrangler dev --env private`: R2, no expiry, Access required, with a stand-in for Access in front (`stand_in.rs`) | `DRAFFTINK_PARITY_WRANGLER=1` |
//!
//! A scenario never asks which target it runs on. What legitimately differs (how long a room's deletion takes to show)
//! is data of the target (`Relay::slack`). A scenario that needs a short room lifetime asks for [`Lifetime::Short`], and
//! every target is started with the same numbers.
//!
//! ```text
//! cargo nextest run -p drafftink-server parity                               # in_process and child; wrangler prints SKIPPED
//! DRAFFTINK_PARITY_WRANGLER=1 cargo nextest run -p drafftink-server parity   # all three (needs Node 22 and the built wasm)
//! cargo nextest run -p drafftink-server parity::wrangler::expire              # one scenario on one target
//! ```
//!
//! Wrangler needs `cd apps/drafftink-worker && npm ci && npm run build:wasm` once. Its output goes to
//! `target/parity-wrangler/<test>.log`, which is printed when the test fails.

#[path = "../common/mod.rs"]
mod common;

mod child;
mod in_process;
mod raw;
mod relay;
mod scenarios;
mod scene;
mod stand_in;
mod wrangler;

use relay::{Lifetime, Target};

/// The tests every target has, one per scenario. The tests are named `parity::<target>::<scenario>`.
macro_rules! common_scenarios {
    ($target:expr) => {
        #[test]
        fn join() {
            run($target, Lifetime::Normal, scenarios::join);
        }
        #[test]
        fn edit() {
            run($target, Lifetime::Normal, scenarios::edit);
        }
        #[test]
        fn offline() {
            run($target, Lifetime::Normal, scenarios::offline);
        }
        #[test]
        fn images() {
            run($target, Lifetime::Normal, scenarios::images);
        }
        #[test]
        fn membership() {
            run($target, Lifetime::Normal, scenarios::membership);
        }
        #[test]
        fn hibernate() {
            run($target, Lifetime::Normal, scenarios::hibernate);
        }
        #[test]
        fn canary() {
            run($target, Lifetime::Normal, scenarios::canary);
        }
    };
}

/// A target whose rooms expire: the common scenarios and `expire`.
macro_rules! expiring_target {
    ($target:expr) => {
        common_scenarios!($target);
        #[test]
        fn expire() {
            run($target, Lifetime::Short, scenarios::expire);
        }
    };
}

/// A target whose rooms never expire: the common scenarios, `persist` instead of `expire`, `delete`, and the gate.
macro_rules! persistent_target {
    ($target:expr) => {
        common_scenarios!($target);
        #[test]
        fn persist() {
            run($target, Lifetime::Normal, scenarios::persist);
        }
        #[test]
        fn delete() {
            run($target, Lifetime::Normal, scenarios::delete);
        }
        #[test]
        fn access() {
            run($target, Lifetime::Normal, scenarios::access);
        }
    };
}

/// Starts the target (a gated one that is off prints its `SKIPPED:` line and the test passes) and runs the scenario.
fn run(target: Target, life: Lifetime, scenario: fn(&mut relay::Relay)) {
    if let Some(mut relay) = target.start(life) {
        eprintln!("parity: {} relay on {}", relay.name, relay.addr);
        scenario(&mut relay);
    }
}

mod parity {
    use super::*;

    mod in_process {
        use super::*;
        expiring_target!(Target::InProcess);
    }

    mod child {
        use super::*;
        expiring_target!(Target::Child);
    }

    mod wrangler {
        use super::*;
        expiring_target!(Target::Wrangler);
    }

    mod private {
        use super::*;
        persistent_target!(Target::Private);
    }
}
