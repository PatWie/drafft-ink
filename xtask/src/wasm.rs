//! `cargo xtask wasm`: every crate at L0 to L5 (plus the apps in `WASM_EXTRA`) must compile and
//! pass clippy `-D warnings` for wasm32-unknown-unknown (`cfg(target_arch = "wasm32")` code is
//! not seen by the native clippy gate).

use std::path::Path;

use crate::cargo;
use crate::layers::{Class, TABLE, WASM_EXTRA, WASM_MAX_LAYER};

pub const TARGET: &str = "wasm32-unknown-unknown";

/// Packages to check: every library at `WASM_MAX_LAYER` or below, then `WASM_EXTRA`.
pub fn packages() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = TABLE
        .iter()
        .filter(|(_, class)| matches!(class, Class::Layer(l) if *l <= WASM_MAX_LAYER))
        .map(|(name, _)| *name)
        .collect();
    out.extend(WASM_EXTRA);
    out
}

/// `cargo <subcommand> --target wasm32 --all-features -p <every package>` plus `tail`.
fn command(root: &Path, subcommand: &str, tail: &[&str]) -> std::process::Command {
    let mut cmd = cargo(root);
    cmd.args([subcommand, "--target", TARGET, "--all-features"]);
    for package in packages() {
        cmd.args(["-p", package]);
    }
    cmd.args(tail);
    cmd
}

pub fn run(root: &Path) -> Result<(), String> {
    let missing_target =
        |e: String| format!("{e}\nIf the target is missing: rustup target add {TARGET} (rust-toolchain.toml lists it)");
    crate::run(&mut command(root, "check", &[])).map_err(missing_target)?;
    crate::run(&mut command(root, "clippy", &["--", "-D", "warnings"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_l0_to_l5_and_the_extras_but_not_l6_or_up() {
        let packages = packages();
        for name in [
            "drafftink-geom",
            "drafftink-render",
            "drafftink-doc",
            "drafftink-engine",
            "drafftink-collab",
            "drafftink-web",
        ] {
            assert!(packages.contains(&name), "{name}");
        }
        for name in
            ["drafftink-gpu", "drafftink-ui", "drafftink-bridge", "drafftink-testkit", "drafftink-server", "xtask"]
        {
            assert!(!packages.contains(&name), "{name}");
        }
    }

    #[test]
    fn extras_are_registered() {
        for name in WASM_EXTRA {
            assert!(crate::layers::classify(name).is_some(), "{name}");
        }
    }
}
