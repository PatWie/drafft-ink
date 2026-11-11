//! `cargo xtask media`: renders the README pictures in `docs/img` from the real app, without a window.
//!
//! The work is done by the example `media` of `drafftink-app` (`crates/app/examples/media`), which runs the
//! app headless on a GPU device and writes the PNG files listed in `docs/media/shots.json`. This command builds and
//! runs it. Without a GPU the example uses Mesa lavapipe, the software Vulkan driver, so the pictures do not
//! depend on the machine; set `VK_ICD_FILENAMES` yourself to use another adapter. See `docs/media.md`.

use std::path::Path;

use crate::{cargo, run};

/// Mesa lavapipe's driver file on Debian and Ubuntu.
const LAVAPIPE: &str = "/usr/share/vulkan/icd.d/lvp_icd.json";

/// Entry point of `cargo xtask media [--only NAME]...`. Other arguments go to the example (`--help` lists them).
pub fn run_media(root: &Path, args: &[String]) -> Result<(), String> {
    let mut command = cargo(root);
    command.args(["run", "--package", "drafftink-app", "--example", "media", "--"]).args(args);
    if std::env::var_os("VK_ICD_FILENAMES").is_none() && Path::new(LAVAPIPE).exists() {
        command.env("VK_ICD_FILENAMES", LAVAPIPE);
    }
    run(&mut command)
}
