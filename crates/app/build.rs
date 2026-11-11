//! Records the commit the app is built from, for the About dialog (`DRAFFTINK_BUILD_COMMIT`).
//!
//! `DRAFFTINK_COMMIT` in the environment wins (packaging from a tarball has no `.git`); otherwise the short hash of
//! `HEAD`; otherwise `unknown`. The build never fails because of this.

use std::env;
use std::path::PathBuf;
use std::process::Command;

/// The trimmed standard output of `git <args>`, if git ran and printed something.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok().filter(|o| o.status.success())?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=DRAFFTINK_COMMIT");
    println!("cargo:rerun-if-changed=build.rs");
    // A new commit moves the branch file, not HEAD itself, so watch both.
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap_or_default();
    for path in [git(&["rev-parse", "--git-path", "HEAD"]), git(&["symbolic-ref", "-q", "HEAD"]).and_then(|r| git(&["rev-parse", "--git-path", &r]))]
        .into_iter()
        .flatten()
    {
        println!("cargo:rerun-if-changed={}", manifest_dir.join(path).display());
    }
    let commit = env::var("DRAFFTINK_COMMIT")
        .ok()
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty())
        .or_else(|| git(&["rev-parse", "--short=10", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=DRAFFTINK_BUILD_COMMIT={commit}");
}
