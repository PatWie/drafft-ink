//! Fails the build with a plain message when the feature `hand-fonts` is on and the font files are not there.
//!
//! The GelPen, GelPen Serif and Vanilla Extract files are by Shara Weber and are not part of the package on crates.io
//! (`exclude` in `Cargo.toml`). They exist in the Git repository only, so a build from crates.io with that feature would
//! otherwise stop with "couldn't read ../assets/fonts/GelPen.ttf" from `include_bytes!`.

use std::env;
use std::error::Error;
use std::path::PathBuf;

/// The files the feature embeds, relative to `assets/fonts`.
const HAND_FONT_FILES: [&str; 7] = [
    "GelPen.ttf",
    "GelPenHeavy.ttf",
    "GelPenLight.ttf",
    "GelPenSerifHeavy.ttf",
    "GelPenSerifLight.ttf",
    "GelPenSerifMedium.ttf",
    "VanillaExtract.ttf",
];

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=assets/fonts");
    if env::var_os("CARGO_FEATURE_HAND_FONTS").is_none() {
        return Ok(());
    }
    let fonts = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("assets/fonts");
    let missing: Vec<&str> = HAND_FONT_FILES.into_iter().filter(|name| !fonts.join(name).is_file()).collect();
    if missing.is_empty() {
        return Ok(());
    }
    println!(
        "cargo::error=drafftink-typeset: the feature `hand-fonts` needs the hand-drawn font files, and these are missing from {}: {}. \
         These fonts (GelPen, GelPen Serif, Vanilla Extract, by Shara Weber) are not part of the crates.io packages; they \
         are in the Git repository (https://github.com/PatWie/drafft-ink). Build from a checkout, or leave the feature \
         off to use Noto Sans instead.",
        fonts.display(),
        missing.join(", ")
    );
    Err("the hand-drawn font files are missing".into())
}
