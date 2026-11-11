//! The engine (L5) stays free of UI, OS and GPU dependencies.
//!
//! Two checks read the crate's own files at run time:
//!
//! 1. `Cargo.toml` names no UI, windowing, GPU, async-runtime or browser crate.
//! 2. No line of code under `src/` uses such a crate, the file system, the network, processes, sleeping
//!    or the wall clock. The engine reaches the outside world only through the traits in `src/ports.rs`.
//!
//! `cargo xtask layers` checks the same rules for the whole workspace from `cargo metadata`. This test
//! is the engine-local backstop that also looks at source text, which metadata cannot see (`std::fs`).
//!
//! The scanner is line based and deliberately simple. It skips lines that start with `//` (comments and
//! doc comments). It does not understand block comments or string literals, so a token inside either
//! is reported. Fix the code, or tell the owner of this test, rather than weakening the lists.

use std::fs;
use std::path::{Path, PathBuf};

/// A dependency whose name equals or starts with one of these is refused.
const FORBIDDEN_DEPS: &[&str] = &[
    "egui",
    "eframe",
    "egui_",
    "winit",
    "rfd",
    "arboard",
    "accesskit",
    "bevy",
    "wgpu",
    "vello",
    "drafftink-ui",
    "drafftink-gpu",
    "drafftink-bridge",
    "drafftink-testkit",
    "drafftink-render",
    "tokio",
    "web-sys",
    "js-sys",
    "wasm-bindgen",
];

/// Source tokens that a non-comment line of the engine must not contain.
const FORBIDDEN_TOKENS: &[&str] = &[
    "egui::",
    "eframe::",
    "winit::",
    "rfd::",
    "arboard::",
    "wgpu::",
    "vello::",
    "accesskit",
    "std::fs",
    "std::net",
    "std::process",
    "std::thread::sleep",
    "Instant::now",
    "SystemTime::now",
    "web_sys",
    "js_sys",
];

const DEP_KINDS: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

fn engine_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

// ---- Cargo.toml ----

/// Where a manifest line sits.
#[derive(Debug, PartialEq, Eq)]
enum Section {
    /// Not a dependency table.
    Other,
    /// `[dependencies]` and friends: each `name = ...` line is a dependency.
    List,
    /// `[dependencies.name]`: the header names the dependency.
    Single(String),
}

fn classify(header: &str) -> Section {
    let h = header.trim().trim_start_matches('[').trim_end_matches(']').trim();
    let target = h.starts_with("target.");
    for kind in DEP_KINDS {
        if h == *kind || (target && h.ends_with(&format!(".{kind}"))) {
            return Section::List;
        }
        let dotted = format!("{kind}.");
        let at = if let Some(rest) = h.strip_prefix(&dotted) {
            Some(h.len() - rest.len())
        } else if target {
            h.rfind(&format!(".{dotted}")).map(|p| p + 1 + dotted.len())
        } else {
            None
        };
        if let Some(start) = at {
            let name = h.get(start..).unwrap_or_default().trim_matches(|c| c == '"' || c == '\'');
            return Section::Single(name.to_owned());
        }
    }
    Section::Other
}

/// The dependency name at the start of a `name = ...` or `name.workspace = true` line.
fn dep_name(line: &str) -> Option<String> {
    let end = line.find(|c: char| c == '=' || c == '.' || c.is_whitespace())?;
    let name = line.get(..end)?.trim_matches(|c| c == '"' || c == '\'');
    (!name.is_empty()).then(|| name.to_owned())
}

/// The crate named by `package = "x"` in a line, which renames a dependency.
fn package_rename(line: &str) -> Option<String> {
    let rest = line.split("package").nth(1)?.trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    rest.split('"').next().map(str::to_owned)
}

fn forbidden_dep(name: &str) -> Option<&'static str> {
    FORBIDDEN_DEPS.iter().copied().find(|bad| name == *bad || name.starts_with(bad))
}

/// Every forbidden dependency in the text of a manifest, as `line N: message`.
fn manifest_hits(manifest: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let mut section = Section::Other;
    for (i, raw) in manifest.lines().enumerate() {
        let line = raw.trim();
        let n = i + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            section = classify(line);
            if let Section::Single(name) = &section
                && let Some(bad) = forbidden_dep(name)
            {
                hits.push(format!("line {n}: dependency `{name}` (matches `{bad}`)"));
            }
            continue;
        }
        let mut names: Vec<String> = Vec::new();
        match &section {
            Section::List => names.extend(dep_name(line)),
            Section::Single(_) => {}
            Section::Other => continue,
        }
        names.extend(package_rename(line));
        for name in names {
            if let Some(bad) = forbidden_dep(&name) {
                hits.push(format!("line {n}: dependency `{name}` (matches `{bad}`)"));
            }
        }
    }
    hits
}

// ---- sources ----

/// Every forbidden token on a non-comment line of `text`, as `label:line: token ...`.
fn source_hits(label: &str, text: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for token in FORBIDDEN_TOKENS {
            if line.contains(token) {
                hits.push(format!("{label}:{}: `{token}` in `{}`", i + 1, line.trim()));
            }
        }
    }
    hits
}

/// Every `.rs` file under `dir`, sorted, so reports are stable.
fn rust_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("cannot list {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            out.extend(rust_files(&path)?);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(out)
}

fn scan_engine_sources() -> Result<(usize, Vec<String>), String> {
    let root = engine_dir();
    let src = root.join("src");
    let files = rust_files(&src)?;
    let mut hits = Vec::new();
    for file in &files {
        // `*_tests.rs` files are compiled only under `#[cfg(test)]`; some read docs/commands.md from disk.
        if file.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("_tests.rs")) {
            continue;
        }
        let rel = file.strip_prefix(&root).unwrap_or(file).display().to_string();
        hits.extend(source_hits(&rel, &read(file)?));
    }
    Ok((files.len(), hits))
}

// ---- tests ----

#[test]
fn manifest_has_no_ui_os_or_gpu_dependency() -> Result<(), String> {
    let manifest = read(&engine_dir().join("Cargo.toml"))?;
    assert!(
        manifest.contains("[dependencies]"),
        "Cargo.toml has no [dependencies] section; the parser would check nothing"
    );
    let hits = manifest_hits(&manifest);
    assert!(
        hits.is_empty(),
        "crates/engine/Cargo.toml depends on a crate the engine must not use (docs/architecture.md rules 6, 7 and 9):\n  {}",
        hits.join("\n  ")
    );
    Ok(())
}

#[test]
fn sources_reach_the_outside_world_only_through_ports() -> Result<(), String> {
    let (files, hits) = scan_engine_sources()?;
    assert!(files >= 10, "found only {files} files under crates/engine/src; the walk is broken");
    assert!(
        hits.is_empty(),
        "the engine uses a UI, OS or GPU API directly. Go through a trait in src/ports.rs instead:\n  {}",
        hits.join("\n  ")
    );
    Ok(())
}

#[test]
fn scanner_flags_code_and_ignores_comments() {
    let text = "use egui::Ui;\n// egui:: in a comment\n    /// std::fs in a doc comment\n//! winit:: in a module doc\nlet t = Instant::now();\nlet ok = 1;\n";
    let hits = source_hits("synthetic.rs", text);
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(hits.iter().any(|h| h.starts_with("synthetic.rs:1:") && h.contains("egui::")), "{hits:?}");
    assert!(hits.iter().any(|h| h.starts_with("synthetic.rs:5:") && h.contains("Instant::now")), "{hits:?}");

    // Every listed token is caught on a code line.
    for token in FORBIDDEN_TOKENS {
        let hits = source_hits("t.rs", &format!("let x = {token};"));
        assert_eq!(hits.len(), 1, "token `{token}` was not flagged: {hits:?}");
    }
}

#[test]
fn manifest_parser_flags_every_dependency_form() {
    let manifest = r#"
[package]
name = "demo"

[dependencies]
serde.workspace = true
egui = "0.30"
winit = { version = "0.30" }
"egui_extras" = "0.30"
fine = { package = "wgpu", version = "1" }

[dev-dependencies]
tokio = { workspace = true }
drafftink-testkit = { path = "../testkit" }

[build-dependencies.vello_shaders]
version = "0.1"

[target.'cfg(target_arch = "wasm32")'.dependencies]
web-sys = "0.3"

[dependencies.wasm-bindgen]
version = "0.2"

[workspace.dependencies]
egui = "0.30"
"#;
    let hits = manifest_hits(manifest);
    let named = |n: &str| hits.iter().any(|h| h.contains(&format!("`{n}`")));
    for n in [
        "egui",
        "winit",
        "egui_extras",
        "wgpu",
        "tokio",
        "drafftink-testkit",
        "vello_shaders",
        "web-sys",
        "wasm-bindgen",
    ] {
        assert!(named(n), "`{n}` was not flagged: {hits:?}");
    }
    assert!(!named("serde"), "{hits:?}");
    // `[workspace.dependencies]` and `[package]` are not dependency tables of this crate.
    assert_eq!(hits.len(), 9, "{hits:?}");

    let clean = "[dependencies]\ndrafftink-scene = { path = \"../scene\" }\nserde.workspace = true\n# egui = \"1\"\n";
    assert!(manifest_hits(clean).is_empty());
}
