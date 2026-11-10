//! External cargo tools that gates need (cargo-nextest, cargo-deny), installed into `~/.cargo` on
//! first use.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::{cargo, run};

/// Whether `cargo <subcommand> --version` works.
pub fn has_subcommand(root: &Path, subcommand: &str) -> bool {
    cargo(root)
        .args([subcommand, "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Make sure `cargo <subcommand>` exists, installing `krate` with `cargo install --locked` if not.
pub fn ensure(root: &Path, subcommand: &str, krate: &str, version: Option<&str>) -> Result<(), String> {
    if has_subcommand(root, subcommand) {
        return Ok(());
    }
    println!("cargo-{subcommand} is not installed; installing {krate} (one time)");
    let mut install = cargo(root);
    install.args(["install", krate, "--locked"]);
    if let Some(version) = version {
        install.args(["--version", version]);
    }
    run(&mut install)?;
    if has_subcommand(root, subcommand) {
        Ok(())
    } else {
        Err(format!("installed {krate}, but `cargo {subcommand} --version` still fails; is ~/.cargo/bin on PATH?"))
    }
}

/// `git` output as a string, or `None` when git fails.
pub fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").current_dir(root).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}
