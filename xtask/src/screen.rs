//! The programs that must not have a screen (`cargo xtask layers`).
//!
//! `drafftink-headless` runs on servers, in CI and in containers. It links no GPU, window, UI toolkit or file-dialog
//! code, directly or through another crate. The layering table cannot see that: it checks the edges between our own
//! crates and the direct dependencies of each, while a screen crate can arrive three levels down. This check reads the
//! resolved graph of the program (`cargo tree -e normal`, every feature, every target) and names each forbidden crate
//! together with the way it got in.

use std::path::Path;

use crate::cargo;

/// Packages that are built without a screen.
pub const HEADLESS_PROGRAMS: &[&str] = &["drafftink-headless"];

/// The crates a headless program must not link. A trailing `*` makes the entry a prefix.
pub const SCREEN_CRATES: &[&str] =
    &["drafftink-gpu", "drafftink-ui", "drafftink-app", "wgpu*", "winit", "egui*", "eframe", "rfd"];

fn is_screen_crate(name: &str) -> bool {
    SCREEN_CRATES.iter().any(|pattern| match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *pattern,
    })
}

/// One line of `cargo tree --prefix depth --format "|{p}"`: how deep it is, and the crate. The `|` keeps the depth apart
/// from a crate name that starts with a digit.
fn parse_line(line: &str) -> Option<(usize, &str)> {
    let (depth, package) = line.split_once('|')?;
    Some((depth.trim().parse().ok()?, package.split_whitespace().next()?))
}

/// The screen crates in the output of `cargo tree --prefix depth`, each with the path from the program down to it.
pub fn find_screen_crates(tree: &str) -> Vec<String> {
    let mut path: Vec<&str> = Vec::new();
    let mut found: Vec<String> = Vec::new();
    for (depth, name) in tree.lines().filter_map(parse_line) {
        path.truncate(depth);
        path.push(name);
        if is_screen_crate(name) {
            let chain = path.join(" -> ");
            if !found.contains(&chain) {
                found.push(chain);
            }
        }
    }
    found
}

fn tree_of(root: &Path, package: &str) -> Result<String, String> {
    let output = cargo(root)
        .args(["tree", "-p", package, "-e", "normal", "--all-features", "--target", "all", "--prefix", "depth"])
        .args(["--format", "|{p}"])
        .output()
        .map_err(|e| format!("cannot run cargo tree: {e}"))?;
    if !output.status.success() {
        return Err(format!("cargo tree -p {package} failed:\n{}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Checks every headless program. The error lists each way a screen crate is reached.
pub fn run(root: &Path) -> Result<(), String> {
    let mut problems = Vec::new();
    for package in HEADLESS_PROGRAMS {
        let found = find_screen_crates(&tree_of(root, package)?);
        problems.extend(found.into_iter().map(|chain| format!("{package} links a screen crate: {chain}")));
    }
    if problems.is_empty() {
        println!(
            "screen: no GPU, window, UI or file-dialog crate in the dependencies of {}",
            HEADLESS_PROGRAMS.join(", ")
        );
        return Ok(());
    }
    Err(format!(
        "{}\nA program without a screen must not depend on {}. Move the code that needs one to the desktop app.",
        problems.join("\n"),
        SCREEN_CRATES.join(", ")
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const CLEAN: &str = "0|drafftink-headless v0.2.0 (/x)\n1|drafftink-cli v0.2.0 (/x)\n2|drafftink-engine v0.2.0 (/x)\n1|clap v4.5.0\n1|drafftink-server v0.2.0 (/x)\n2|axum v0.8.0\n";

    #[test]
    fn a_graph_without_a_screen_passes() {
        assert!(find_screen_crates(CLEAN).is_empty());
    }

    #[test]
    fn a_screen_crate_far_down_is_found_with_its_path() {
        let tree = format!(
            "{CLEAN}1|drafftink-mcp v0.2.0 (/x)\n2|drafftink-bridge v0.2.0 (/x)\n3|drafftink-ui v0.2.0 (/x)\n4|egui v0.37.0\n"
        );
        assert_eq!(
            find_screen_crates(&tree),
            [
                "drafftink-headless -> drafftink-mcp -> drafftink-bridge -> drafftink-ui",
                "drafftink-headless -> drafftink-mcp -> drafftink-bridge -> drafftink-ui -> egui",
            ]
        );
    }

    #[test]
    fn every_named_crate_and_each_prefix_is_a_screen_crate() {
        for name in [
            "drafftink-gpu",
            "drafftink-ui",
            "drafftink-app",
            "wgpu",
            "wgpu-hal",
            "winit",
            "egui",
            "egui-wgpu",
            "egui_glow",
            "eframe",
            "rfd",
        ] {
            assert!(is_screen_crate(name), "{name}");
        }
        for name in ["drafftink-bridge", "drafftink-engine", "winnow", "tokio", "image", "rfc6979"] {
            assert!(!is_screen_crate(name), "{name}");
        }
    }

    fn workspace_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_default()
    }

    /// The parser against the real output of `cargo tree`: the desktop app must show up with a screen, so a silent
    /// change of the output format cannot make the check pass for the wrong reason.
    #[test]
    fn the_desktop_app_has_a_screen_and_the_headless_program_has_none() {
        let root = workspace_root();
        let desktop = find_screen_crates(&tree_of(&root, "drafftink").unwrap());
        for want in ["drafftink-gpu", "drafftink-ui", "egui", "winit", "wgpu"] {
            assert!(desktop.iter().any(|chain| chain.ends_with(want)), "{want} not found in {desktop:?}");
        }
        assert!(find_screen_crates(&tree_of(&root, "drafftink-headless").unwrap()).is_empty());
    }

    #[test]
    fn a_crate_name_that_starts_with_a_digit_keeps_its_name() {
        assert_eq!(parse_line("12|7z v1.0.0"), Some((12, "7z")));
        assert_eq!(parse_line("1|egui v0.37.0 (*)"), Some((1, "egui")));
        assert_eq!(parse_line("a line without a depth"), None);
    }

    #[test]
    fn the_same_chain_is_reported_once() {
        let tree = "0|drafftink-headless v1 (/x)\n1|winit v0.30\n1|winit v0.30\n";
        assert_eq!(find_screen_crates(tree), ["drafftink-headless -> winit"]);
    }
}
