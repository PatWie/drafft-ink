//! Dependency layering rules (docs/architecture.md).
//!
//! The rule engine works on a small model that does not depend on cargo, so unit tests can use
//! synthetic graphs. `from_metadata` builds the model from `cargo metadata`.
//!
//! Adapted from solidcraft's xtask (MIT OR Apache-2.0); see ATTRIBUTION.md.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// Where a workspace crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A library crate at layer 0 to 6. It may depend on workspace crates of strictly lower layers.
    Layer(u8),
    /// The test kit, layer 7. It may use any library. Other crates may use it only as a dev-dependency.
    Testkit,
    /// An application, layer 8. It may use any library. Nothing may depend on an app.
    App,
    /// Build tooling (xtask). It uses no workspace crate and no workspace crate uses it.
    Tool,
    /// A copy of an upstream crate in `third_party/`, published under our name because crates.io has no Git
    /// dependencies. It is a leaf: it uses only other vendored crates, and its external dependencies are not held
    /// to the allowlist (they are upstream's).
    Vendored,
}

pub const TESTKIT_LAYER: u8 = 7;
pub const APP_LAYER: u8 = 8;
/// Everything below this layer builds for wasm32-unknown-unknown (`cargo xtask wasm`).
pub const WASM_MAX_LAYER: u8 = 5;

impl Class {
    pub fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) => Some(l),
            Class::Testkit => Some(TESTKIT_LAYER),
            Class::App => Some(APP_LAYER),
            Class::Tool | Class::Vendored => None,
        }
    }
}

/// The layering table: every workspace package, by package name.
pub const TABLE: &[(&str, Class)] = &[
    // L0: plain data and math, no workspace dependencies
    ("drafftink-geom", Class::Layer(0)),
    ("drafftink-profile", Class::Layer(0)),
    ("drafftink-protocol", Class::Layer(0)),
    // L1
    ("drafftink-scene", Class::Layer(1)),
    ("drafftink-typeset", Class::Layer(1)),
    ("drafftink-sketch", Class::Layer(1)),
    // L2
    ("drafftink-render", Class::Layer(2)),
    // L3
    ("drafftink-canvas", Class::Layer(3)),
    ("drafftink-doc", Class::Layer(3)),
    // L4
    ("drafftink-edit", Class::Layer(4)),
    ("drafftink-io", Class::Layer(4)),
    ("drafftink-collab", Class::Layer(4)),
    // L5
    ("drafftink-engine", Class::Layer(5)),
    // L6
    ("drafftink-gpu", Class::Layer(6)),
    ("drafftink-ui", Class::Layer(6)),
    ("drafftink-bridge", Class::Layer(6)),
    // L7
    ("drafftink-testkit", Class::Testkit),
    // The application glue: uses any library below it, and only the apps use it.
    ("drafftink-app", Class::Layer(7)),
    // apps
    ("drafftink", Class::App),
    ("drafftink-web", Class::App),
    ("drafftink-canvas-js", Class::App),
    ("drafftink-server", Class::App),
    ("drafftink-cli", Class::App),
    ("drafftink-mcp", Class::App),
    ("drafftink-e2e", Class::App),
    // tooling
    ("xtask", Class::Tool),
    // vendored upstream crates (third_party/, docs/typeset.md)
    ("drafftink-rex", Class::Vendored),
    ("drafftink-unicode-math", Class::Vendored),
];

/// Apps (and other non-library packages) that also build for wasm32-unknown-unknown, on top of
/// every crate at L0 to L5.
pub const WASM_EXTRA: &[&str] = &["drafftink-app", "drafftink-web", "drafftink-canvas-js"];

/// A same-layer edge that is allowed, with the reason. Earlier name may be used by the later one.
/// Keep this list short: each entry is a hole in the layering.
pub struct SameLayerEdge {
    pub lower: &'static str,
    pub upper: &'static str,
    pub reason: &'static str,
}

pub const SAME_LAYER_EDGES: &[SameLayerEdge] = &[
    SameLayerEdge {
        lower: "drafftink-cli",
        upper: "drafftink",
        reason: "`drafftink convert|run|outline|commands` is the code of the standalone drafftink-cli, with no copy",
    },
    SameLayerEdge {
        lower: "drafftink-mcp",
        upper: "drafftink",
        reason: "`drafftink mcp` is the code of the standalone drafftink-mcp, with no copy",
    },
];

/// Restriction on an external crate: where it may appear.
pub struct ExternalRule {
    /// Crate names. A trailing `*` makes the entry a prefix.
    pub names: &'static [&'static str],
    /// First layer that may depend on it.
    pub min_layer: u8,
    /// Workspace crates below `min_layer` that may use it anyway.
    pub exceptions: &'static [&'static str],
    /// Why, for the error message.
    pub why: &'static str,
}

pub const EXTERNAL_RULES: &[ExternalRule] = &[
    ExternalRule {
        names: &["egui*", "eframe", "winit", "rfd", "arboard", "accesskit*", "bevy*"],
        min_layer: 6,
        exceptions: &[],
        why: "UI toolkits, windowing, dialogs and the OS clipboard belong to L6 and the apps; lower layers use ports",
    },
    ExternalRule {
        names: &["wgpu*"],
        min_layer: 6,
        // render may name wgpu types that vello exposes in its public API. It must not create a
        // device or a surface: crates/gpu owns those.
        exceptions: &["drafftink-render"],
        why: "the gpu crate (L6) owns the wgpu device and surface; only render may name wgpu types",
    },
    ExternalRule {
        names: &["vello*"],
        min_layer: 2,
        exceptions: &[],
        why: "Vello scene encoding starts at render (L2)",
    },
];

/// External crates that every library below L6 may use, whatever its layer: plain data and error handling.
pub const COMMON_EXTERNAL: &[&str] = &["serde", "serde_json", "thiserror"];

/// External crates that any library below L6 may use as a dev-dependency (benches, property tests, snapshots,
/// temporary directories). They never ship.
pub const TEST_TOOLING: &[&str] = &["criterion", "proptest", "insta", "tempfile"];

/// The allowlist: which external crates each library below L6 may depend on, on top of [`COMMON_EXTERNAL`] (and,
/// for dev-dependencies, [`TEST_TOOLING`]). Every other external crate is a violation, so a UI toolkit, a window
/// system, a GPU API, a filesystem or HTTP crate cannot arrive by the back door (the denylist in
/// [`EXTERNAL_RULES`] only knows the names someone thought of). A new dependency needs a line here, in the same
/// change, and a reviewer sees it. A line that no longer matches a dependency is a violation too, so the list
/// cannot hold crates a layer stopped using.
///
/// The network, browser and file system crates of `drafftink-collab` (L4) are the only ones below L6; collab owns
/// the connection to a room behind the engine's ports, and the L0 to L5 wasm build (`cargo xtask wasm`) holds it
/// to browser-safe code.
pub const ALLOWED_EXTERNAL: &[(&str, &[&str])] = &[
    // L0
    ("drafftink-geom", &["kurbo", "rstar"]),
    ("drafftink-profile", &["web-time"]),
    ("drafftink-protocol", &[]),
    // L1
    ("drafftink-scene", &["base64", "kurbo", "peniko", "schemars", "sha2", "smallvec", "smol_str"]),
    ("drafftink-typeset", &["kurbo", "parley", "peniko", "png", "ttf-parser"]),
    ("drafftink-sketch", &["getrandom", "kurbo", "roughr"]),
    // L2
    ("drafftink-render", &["base64", "image", "kurbo", "peniko", "png", "pollster", "schemars", "vello"]),
    // L3
    ("drafftink-canvas", &["smallvec"]),
    ("drafftink-doc", &["base64", "flate2", "kurbo", "loro", "smallvec", "xxhash-rust"]),
    // L4
    ("drafftink-edit", &["schemars"]),
    ("drafftink-io", &["base64", "flate2", "image", "kurbo", "loro", "png", "roxmltree", "sha2"]),
    (
        "drafftink-collab",
        &[
            "futures-util",
            "js-sys",
            "log",
            "loro",
            "rustls",
            "rustls-native-certs",
            "tokio",
            "tokio-rustls",
            "tokio-tungstenite",
            "wasm-bindgen",
            "web-sys",
            "webpki-roots",
        ],
    ),
    // L5
    ("drafftink-engine", &["base64", "loro", "png", "schemars", "serde_path_to_error"]),
];

/// The layers whose crates the allowlist covers: everything below L6.
pub const ALLOWLIST_BELOW: u8 = 6;

fn matches_pattern(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

/// The libraries below L6, which `cargo xtask clippy` holds to `xtask/clippy-below-l6/clippy.toml`.
pub fn libraries_below_l6() -> Vec<&'static str> {
    TABLE
        .iter()
        .filter(|(_, class)| matches!(class, Class::Layer(l) if *l < ALLOWLIST_BELOW))
        .map(|(name, _)| *name)
        .collect()
}

pub fn classify(package: &str) -> Option<Class> {
    TABLE.iter().find(|(name, _)| *name == package).map(|(_, class)| *class)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    /// True if the dependency is a workspace member.
    pub workspace: bool,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered {
        krate: String,
    },
    Stale {
        krate: String,
    },
    /// An edge to a higher layer (`to > from`) or to the same layer (`to == from`).
    Edge {
        krate: String,
        dep: String,
        from: u8,
        to: u8,
        kind: DepKind,
    },
    ToolEdge {
        krate: String,
        dep: String,
    },
    /// A vendored crate that uses a crate of ours (or the other way: only vendored crates may use each other).
    VendoredEdge {
        krate: String,
        dep: String,
    },
    TestkitAsNormalDep {
        krate: String,
    },
    External {
        krate: String,
        dep: String,
        layer: u8,
        min_layer: u8,
        why: &'static str,
    },
    /// An external crate that the allowlist of the crate does not name.
    NotAllowed {
        krate: String,
        dep: String,
        layer: u8,
        kind: DepKind,
    },
    /// A library below L6 with no row in [`ALLOWED_EXTERNAL`].
    NoAllowlist {
        krate: String,
    },
    /// An allowlist entry for a crate the library does not depend on.
    UnusedAllowance {
        krate: String,
        dep: String,
    },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => write!(
                f,
                "{krate}: unknown workspace crate; assign it a layer in xtask/src/layers.rs TABLE and docs/architecture.md"
            ),
            Violation::Stale { krate } => {
                write!(f, "{krate}: listed in xtask/src/layers.rs TABLE but not in the workspace; remove the row")
            }
            Violation::Edge { krate, dep, from, to, kind } => {
                let direction = if to == from { "sideways, same layer" } else { "upward" };
                write!(
                    f,
                    "{krate} (L{from}) -> {dep} (L{to}) [{kind:?}]: {direction}; only strictly lower layers are allowed"
                )
            }
            Violation::ToolEdge { krate, dep } => {
                write!(
                    f,
                    "{krate}: xtask is standalone; it may not depend on or be depended on by workspace crate {dep}"
                )
            }
            Violation::VendoredEdge { krate, dep } => {
                write!(
                    f,
                    "{krate}: a vendored crate in third_party/ may only depend on other vendored crates, not on {dep}"
                )
            }
            Violation::TestkitAsNormalDep { krate } => {
                write!(f, "{krate}: drafftink-testkit may only be a dev-dependency")
            }
            Violation::External { krate, dep, layer, min_layer, why } => {
                write!(f, "{krate} (L{layer}) depends on `{dep}`, allowed from L{min_layer}: {why}")
            }
            Violation::NotAllowed { krate, dep, layer, kind } => write!(
                f,
                "{krate} (L{layer}) has the external dependency `{dep}` [{kind:?}], which is not on its allowlist; \
                 if the layer really needs it, add it to ALLOWED_EXTERNAL in xtask/src/layers.rs with the change"
            ),
            Violation::NoAllowlist { krate } => write!(
                f,
                "{krate}: a library below L{ALLOWLIST_BELOW} needs a row in ALLOWED_EXTERNAL in xtask/src/layers.rs"
            ),
            Violation::UnusedAllowance { krate, dep } => {
                write!(f, "{krate}: ALLOWED_EXTERNAL allows `{dep}`, which the crate does not use; remove it")
            }
        }
    }
}

fn same_layer_allowed(from: &str, to: &str) -> bool {
    SAME_LAYER_EDGES.iter().any(|e| e.upper == from && e.lower == to)
}

/// Check every rule. The result is sorted so output is stable.
pub fn check(crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        let Some(class) = classify(&c.name) else {
            out.push(Violation::Unregistered { krate: c.name.clone() });
            continue;
        };
        for d in &c.deps {
            // A dev-dependency on itself (to turn on features in tests) is fine.
            if d.name == c.name {
                continue;
            }
            if d.workspace {
                check_workspace_edge(&mut out, c, class, d);
            } else if let Some(layer) = class.layer() {
                check_external(&mut out, &c.name, layer, &d.name, d.kind);
            }
        }
    }
    out.sort_by_key(|v| v.to_string());
    out.dedup();
    out
}

fn check_workspace_edge(out: &mut Vec<Violation>, c: &Crate, class: Class, d: &Dep) {
    let dep_class = classify(&d.name);
    if class == Class::Tool || dep_class == Some(Class::Tool) {
        out.push(Violation::ToolEdge { krate: c.name.clone(), dep: d.name.clone() });
        return;
    }
    // An unregistered dependency is reported on its own entry.
    let Some(dep_class) = dep_class else { return };
    if class == Class::Vendored && dep_class != Class::Vendored {
        out.push(Violation::VendoredEdge { krate: c.name.clone(), dep: d.name.clone() });
        return;
    }
    let (Some(from), Some(to)) = (class.layer(), dep_class.layer()) else { return };
    if dep_class == Class::Testkit {
        if d.kind != DepKind::Dev {
            out.push(Violation::TestkitAsNormalDep { krate: c.name.clone() });
        }
        // The testkit is L7, above everything but the apps, so dev-use needs no further check.
        // (An app is L8 and may use it; so may every library, as a dev-dependency.)
        return;
    }
    if to < from || (to == from && same_layer_allowed(&c.name, &d.name)) {
        return;
    }
    out.push(Violation::Edge { krate: c.name.clone(), dep: d.name.clone(), from, to, kind: d.kind });
}

fn check_external(out: &mut Vec<Violation>, krate: &str, layer: u8, dep: &str, kind: DepKind) {
    let before = out.len();
    for rule in EXTERNAL_RULES {
        if layer < rule.min_layer
            && rule.names.iter().any(|p| matches_pattern(p, dep))
            && !rule.exceptions.contains(&krate)
        {
            out.push(Violation::External {
                krate: krate.to_owned(),
                dep: dep.to_owned(),
                layer,
                min_layer: rule.min_layer,
                why: rule.why,
            });
        }
    }
    // The denylist message is the more specific one; the allowlist speaks when nothing named the crate.
    if out.len() == before && layer < ALLOWLIST_BELOW {
        let allowed = ALLOWED_EXTERNAL.iter().find(|(name, _)| *name == krate).map(|(_, list)| *list);
        match allowed {
            None => out.push(Violation::NoAllowlist { krate: krate.to_owned() }),
            Some(list) => {
                let ok = COMMON_EXTERNAL.contains(&dep)
                    || list.contains(&dep)
                    || (kind == DepKind::Dev && TEST_TOOLING.contains(&dep));
                if !ok {
                    out.push(Violation::NotAllowed { krate: krate.to_owned(), dep: dep.to_owned(), layer, kind });
                }
            }
        }
    }
}

/// Allowlist entries that name a dependency the crate does not have, and rows for crates that are not in the
/// workspace.
pub fn check_unused_allowances(crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for (krate, list) in ALLOWED_EXTERNAL {
        let Some(c) = crates.iter().find(|c| c.name == *krate) else { continue };
        for dep in *list {
            if !c.deps.iter().any(|d| !d.workspace && d.name == *dep) {
                out.push(Violation::UnusedAllowance { krate: (*krate).to_owned(), dep: (*dep).to_owned() });
            }
        }
    }
    out
}

/// Table rows that name a package the workspace does not have.
pub fn check_stale(crates: &[Crate]) -> Vec<Violation> {
    TABLE
        .iter()
        .filter(|(name, _)| !crates.iter().any(|c| c.name == *name))
        .map(|(name, _)| Violation::Stale { krate: (*name).to_owned() })
        .collect()
}

/// Build the model from `cargo metadata --format-version 1 --no-deps`.
pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let packages = meta["packages"].as_array().ok_or("cargo metadata: no packages array")?;
    let members: Vec<&str> = packages.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in packages {
        let name = p["name"].as_str().ok_or("cargo metadata: package without a name")?.to_owned();
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let dep_name = d["name"].as_str().unwrap_or_default().to_owned();
            let kind = match d["kind"].as_str() {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            // A path dependency that is not a member is not ours to trust: it is external and is held to the allowlist.
            let workspace = members.contains(&dep_name.as_str());
            deps.push(Dep { name: dep_name, kind, workspace });
        }
        out.push(Crate { name, deps });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn describe(class: Option<Class>) -> String {
    match class {
        Some(Class::Layer(l)) => format!("L{l}"),
        Some(Class::Testkit) => format!("L{TESTKIT_LAYER} testkit"),
        Some(Class::App) => format!("L{APP_LAYER} app"),
        Some(Class::Tool) => "tool".into(),
        Some(Class::Vendored) => "vendored".into(),
        None => "UNASSIGNED".into(),
    }
}

/// Load the workspace model with `cargo metadata`.
pub fn load(root: &Path) -> Result<Vec<Crate>, String> {
    let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&output.stderr)));
    }
    let meta: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata output: {e}"))?;
    from_metadata(&meta)
}

/// `cargo xtask layers [--list]`.
pub fn run(root: &Path, list: bool) -> Result<(), String> {
    let crates = load(root)?;
    if list {
        for c in &crates {
            println!("{:<22} {}", c.name, describe(classify(&c.name)));
        }
        for e in SAME_LAYER_EDGES {
            println!("allowed same-layer edge: {} -> {} ({})", e.upper, e.lower, e.reason);
        }
    }
    let mut violations = check(&crates);
    violations.extend(check_stale(&crates));
    violations.extend(check_unused_allowances(&crates));
    if violations.is_empty() {
        println!("layers: {} crates, no violations", crates.len());
        return Ok(());
    }
    for v in &violations {
        eprintln!("  {v}");
    }
    Err(format!("{} layering violation(s); see docs/architecture.md", violations.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use DepKind::{Dev, Normal};

    fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
        Crate {
            name: name.into(),
            deps: deps.iter().map(|(n, k, w)| Dep { name: (*n).into(), kind: *k, workspace: *w }).collect(),
        }
    }

    fn only(violations: &[Violation]) -> &Violation {
        assert_eq!(violations.len(), 1, "{violations:?}");
        &violations[0]
    }

    #[test]
    fn table_has_no_duplicates() {
        for (i, (a, _)) in TABLE.iter().enumerate() {
            assert!(!TABLE[i + 1..].iter().any(|(b, _)| a == b), "{a} is listed twice");
        }
    }

    #[test]
    fn table_matches_the_plan_layers() {
        let layer = |n: &str| classify(n).and_then(Class::layer);
        for n in ["geom", "profile", "protocol"] {
            assert_eq!(layer(&format!("drafftink-{n}")), Some(0));
        }
        for n in ["scene", "typeset", "sketch"] {
            assert_eq!(layer(&format!("drafftink-{n}")), Some(1));
        }
        assert_eq!(layer("drafftink-render"), Some(2));
        assert_eq!(layer("drafftink-canvas"), Some(3));
        assert_eq!(layer("drafftink-doc"), Some(3));
        assert_eq!(layer("drafftink-edit"), Some(4));
        assert_eq!(layer("drafftink-io"), Some(4));
        assert_eq!(layer("drafftink-engine"), Some(5));
        assert_eq!(layer("drafftink-collab"), Some(4));
        for n in ["gpu", "ui", "bridge"] {
            assert_eq!(layer(&format!("drafftink-{n}")), Some(6));
        }
        assert_eq!(layer("drafftink-testkit"), Some(7));
        assert_eq!(layer("drafftink-app"), Some(7));
        assert_eq!(layer("drafftink"), Some(8));
        assert_eq!(layer("xtask"), None);
    }

    #[test]
    fn clean_downward_graph_passes() {
        let g = [
            c("drafftink-geom", &[("kurbo", Normal, false)]),
            c("drafftink-scene", &[("drafftink-geom", Normal, true)]),
            c("drafftink-render", &[("drafftink-scene", Normal, true), ("vello", Normal, false)]),
            c("drafftink-engine", &[("drafftink-render", Normal, true), ("drafftink-testkit", Dev, true)]),
            c("drafftink-ui", &[("drafftink-engine", Normal, true), ("egui", Normal, false)]),
            c("drafftink-testkit", &[("drafftink-ui", Normal, true)]),
            c("drafftink", &[("drafftink-ui", Normal, true), ("eframe", Normal, false)]),
            c("xtask", &[("serde_json", Normal, false)]),
        ];
        assert!(check(&g).is_empty(), "{:?}", check(&g));
    }

    #[test]
    fn upward_edge_flagged() {
        let v = check(&[c("drafftink-doc", &[("drafftink-engine", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 3, to: 5, .. }));
        assert!(v[0].to_string().contains("upward"));
    }

    #[test]
    fn sideways_edge_flagged() {
        let v = check(&[c("drafftink-typeset", &[("drafftink-sketch", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 1, to: 1, .. }));
        assert!(v[0].to_string().contains("sideways"));
    }

    #[test]
    fn upward_and_sideways_dev_and_build_edges_flagged() {
        let v = check(&[c(
            "drafftink-geom",
            &[("drafftink-scene", Dev, true), ("drafftink-profile", DepKind::Build, true)],
        )]);
        assert_eq!(v.len(), 2, "{v:?}");
    }

    #[test]
    fn no_same_layer_exceptions_remain() {
        let v = check(&[c("drafftink-doc", &[("drafftink-canvas", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 3, to: 3, .. }));
        assert!(check(&[c("drafftink-engine", &[("drafftink-collab", Normal, true)])]).is_empty());
        let v = check(&[c("drafftink-collab", &[("drafftink-engine", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 4, to: 5, .. }));
    }

    #[test]
    fn same_layer_allowance_table_names_real_same_layer_crates() {
        for e in SAME_LAYER_EDGES {
            let (lower, upper) = (classify(e.lower).and_then(Class::layer), classify(e.upper).and_then(Class::layer));
            assert!(lower.is_some() && lower == upper, "{} -> {}", e.upper, e.lower);
            assert!(!e.reason.is_empty());
        }
    }

    #[test]
    fn self_dev_dependency_ignored() {
        assert!(check(&[c("drafftink-doc", &[("drafftink-doc", Dev, true)])]).is_empty());
    }

    #[test]
    fn ui_crates_below_l6_flagged() {
        for dep in ["egui", "eframe", "egui-wgpu", "egui_kittest", "winit", "rfd", "arboard", "bevy_ecs"] {
            let v = check(&[c("drafftink-engine", &[(dep, Normal, false)])]);
            assert!(matches!(only(&v), Violation::External { layer: 5, min_layer: 6, .. }), "{dep}: {v:?}");
        }
        // `egui*` is a prefix rule, so egui_extras and friends are covered too.
        assert_eq!(check(&[c("drafftink-engine", &[("egui_extras", Normal, false)])]).len(), 1);
        assert!(check(&[c("drafftink-engine", &[("serde", Normal, false)])]).is_empty());
        assert!(check(&[c("drafftink-ui", &[("winit", Normal, false), ("egui", Normal, false)])]).is_empty());
        assert!(check(&[c("drafftink-testkit", &[("egui_kittest", Dev, false)])]).is_empty());
    }

    #[test]
    fn ui_crates_flagged_as_dev_dependencies_too() {
        let v = check(&[c("drafftink-canvas", &[("egui_kittest", Dev, false)])]);
        assert!(matches!(only(&v), Violation::External { .. }));
    }

    #[test]
    fn wgpu_only_in_render_and_l6_up() {
        for krate in ["drafftink-geom", "drafftink-scene", "drafftink-canvas", "drafftink-engine"] {
            let v = check(&[c(krate, &[("wgpu", Normal, false)])]);
            assert!(matches!(only(&v), Violation::External { min_layer: 6, .. }), "{krate}");
        }
        // render may name wgpu types in principle (the denylist exception), but its allowlist does not list them.
        let v = check(&[c("drafftink-render", &[("wgpu", Normal, false)])]);
        assert!(matches!(only(&v), Violation::NotAllowed { .. }), "{v:?}");
        assert!(check(&[c("drafftink-gpu", &[("wgpu", Normal, false)])]).is_empty());
        assert!(check(&[c("drafftink-web", &[("wgpu", Normal, false)])]).is_empty());
    }

    #[test]
    fn vello_starts_at_render() {
        for krate in ["drafftink-geom", "drafftink-scene", "drafftink-sketch"] {
            let v = check(&[c(krate, &[("vello", Normal, false)])]);
            assert!(matches!(only(&v), Violation::External { min_layer: 2, .. }), "{krate}");
        }
        assert!(check(&[c("drafftink-render", &[("vello", Normal, false)])]).is_empty());
        let v = check(&[c("drafftink-canvas", &[("vello", Normal, false)])]);
        assert!(matches!(only(&v), Violation::NotAllowed { .. }), "canvas is L3 but does not use vello: {v:?}");
    }

    /// The second clippy pass reads this file; losing a line would silently let I/O into the libraries.
    #[test]
    fn the_strict_clippy_config_bans_io_threads_and_the_wall_clock_below_l6() {
        let config = include_str!("../clippy-below-l6/clippy.toml");
        for path in [
            "std::fs::read",
            "std::fs::read_to_string",
            "std::fs::write",
            "std::fs::File",
            "std::net::TcpStream",
            "std::net::TcpListener",
            "std::thread::spawn",
            "std::time::Instant::now",
            "std::time::Instant",
            "std::process::Command",
            "egui::Context",
            "rfd::FileDialog",
            "web_sys::Window",
        ] {
            assert!(config.contains(&format!("path = \"{path}\"")), "xtask/clippy-below-l6/clippy.toml lacks {path}");
        }
        assert!(libraries_below_l6().contains(&"drafftink-engine") && !libraries_below_l6().contains(&"drafftink-gpu"));
    }

    #[test]
    fn every_library_below_l6_has_an_allowlist_row_and_every_row_a_library() {
        for (name, class) in TABLE {
            if class.layer().is_some_and(|l| l < ALLOWLIST_BELOW) && matches!(class, Class::Layer(_)) {
                assert!(ALLOWED_EXTERNAL.iter().any(|(n, _)| n == name), "{name} has no row in ALLOWED_EXTERNAL");
            }
        }
        for (name, list) in ALLOWED_EXTERNAL {
            let layer = classify(name).and_then(Class::layer);
            assert!(layer.is_some_and(|l| l < ALLOWLIST_BELOW), "{name} is not a library below L{ALLOWLIST_BELOW}");
            assert!(list.windows(2).all(|w| w[0] < w[1]), "{name}: keep the list sorted and free of repeats");
        }
    }

    #[test]
    fn the_allowlist_refuses_crates_the_denylist_never_heard_of() {
        // None of these names is in EXTERNAL_RULES: a denylist lets them through.
        for (krate, dep) in [
            ("drafftink-engine", "epaint"),
            ("drafftink-engine", "web-sys"),
            ("drafftink-engine", "tokio"),
            ("drafftink-scene", "native-dialog"),
            ("drafftink-edit", "tao"),
            ("drafftink-io", "reqwest"),
            ("drafftink-doc", "iced"),
            ("drafftink-geom", "notify"),
        ] {
            let v = check(&[c(krate, &[(dep, Normal, false)])]);
            assert!(matches!(only(&v), Violation::NotAllowed { .. }), "{krate} -> {dep}: {v:?}");
            assert!(v[0].to_string().contains("ALLOWED_EXTERNAL"));
        }
        // Each layer keeps what it needs.
        assert!(check(&[c("drafftink-collab", &[("tokio", Normal, false), ("web-sys", Normal, false)])]).is_empty());
        assert!(check(&[c("drafftink-engine", &[("serde", Normal, false), ("loro", Normal, false)])]).is_empty());
    }

    #[test]
    fn test_tooling_is_allowed_only_as_a_dev_dependency() {
        assert!(check(&[c("drafftink-engine", &[("proptest", Dev, false)])]).is_empty());
        let v = check(&[c("drafftink-engine", &[("proptest", Normal, false)])]);
        assert!(matches!(only(&v), Violation::NotAllowed { kind: Normal, .. }));
    }

    #[test]
    fn the_denylist_message_wins_when_both_rules_name_a_crate() {
        let v = check(&[c("drafftink-engine", &[("egui", Normal, false)])]);
        assert!(matches!(only(&v), Violation::External { .. }), "{v:?}");
    }

    #[test]
    fn a_path_dependency_outside_the_workspace_is_external() {
        let meta: Value = serde_json::from_str(
            r#"{"packages":[
                {"name":"drafftink-geom","dependencies":[
                    {"name":"vendored-ui","kind":null,"path":"/elsewhere/vendored-ui"}]}
            ]}"#,
        )
        .unwrap();
        let g = from_metadata(&meta).unwrap();
        assert!(!g[0].deps[0].workspace);
        assert!(matches!(only(&check(&g)), Violation::NotAllowed { .. }));
    }

    #[test]
    fn an_allowance_nobody_uses_is_reported() {
        let v = check_unused_allowances(&[c("drafftink-geom", &[("kurbo", Normal, false)])]);
        assert_eq!(v, [Violation::UnusedAllowance { krate: "drafftink-geom".into(), dep: "rstar".into() }]);
        assert!(v[0].to_string().contains("remove it"));
    }

    #[test]
    fn unregistered_crate_is_error() {
        let v = check(&[c("drafftink-mystery", &[])]);
        assert!(matches!(only(&v), Violation::Unregistered { krate } if krate == "drafftink-mystery"));
        assert!(v[0].to_string().contains("assign it a layer"));
    }

    #[test]
    fn stale_table_row_is_error() {
        let v = check_stale(&[c("drafftink-geom", &[])]);
        assert_eq!(v.len(), TABLE.len() - 1);
        assert!(v.iter().all(|v| matches!(v, Violation::Stale { .. })));
    }

    #[test]
    fn testkit_only_as_dev_dependency() {
        let v = check(&[c("drafftink-doc", &[("drafftink-testkit", Normal, true)])]);
        assert!(matches!(only(&v), Violation::TestkitAsNormalDep { .. }));
        assert!(check(&[c("drafftink-doc", &[("drafftink-testkit", Dev, true)])]).is_empty());
        assert!(check(&[c("drafftink-testkit", &[("drafftink-engine", Normal, true)])]).is_empty());
        assert!(check(&[c("drafftink-cli", &[("drafftink-testkit", Dev, true)])]).is_empty());
    }

    #[test]
    fn app_glue_sits_between_the_libraries_and_the_apps() {
        // It may use every library below it (gpu, ui, bridge are L6) and the apps may use it.
        let g = [
            c("drafftink-gpu", &[]),
            c(
                "drafftink-app",
                &[("drafftink-gpu", Normal, true), ("drafftink-testkit", Dev, true), ("egui", Normal, false)],
            ),
            c("drafftink", &[("drafftink-app", Normal, true)]),
        ];
        assert!(check(&g).is_empty(), "{:?}", check(&g));
        // No library below it may use it, and it is not an app, so apps and the glue stay apart.
        for krate in ["drafftink-gpu", "drafftink-ui", "drafftink-engine"] {
            let v = check(&[c(krate, &[("drafftink-app", Normal, true)])]);
            assert!(matches!(only(&v), Violation::Edge { to: 7, .. }), "{krate}: {v:?}");
        }
        // The testkit is at the same level and may not use it either.
        let v = check(&[c("drafftink-testkit", &[("drafftink-app", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 7, to: 7, .. }));
        let v = check(&[c("drafftink-app", &[("drafftink", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 7, to: 8, .. }));
    }

    #[test]
    fn testkit_may_not_use_apps() {
        let v = check(&[c("drafftink-testkit", &[("drafftink-cli", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 7, to: 8, .. }));
    }

    #[test]
    fn apps_may_use_libraries_but_not_each_other() {
        assert!(
            check(&[c("drafftink-cli", &[("drafftink-engine", Normal, true), ("drafftink-io", Normal, true)])])
                .is_empty()
        );
        let v = check(&[c("drafftink-mcp", &[("drafftink-cli", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 8, to: 8, .. }));
    }

    #[test]
    fn the_desktop_app_may_use_the_cli_and_mcp_apps_and_nothing_else_may() {
        let ok = [
            c("drafftink-cli", &[]),
            c("drafftink-mcp", &[]),
            c("drafftink", &[("drafftink-cli", Normal, true), ("drafftink-mcp", Normal, true)]),
        ];
        assert!(check(&ok).is_empty(), "{:?}", check(&ok));
        // The edge is one way, and the other apps do not get it.
        let v = check(&[c("drafftink-cli", &[("drafftink", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 8, to: 8, .. }));
        let v = check(&[c("drafftink-server", &[("drafftink-cli", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 8, to: 8, .. }));
    }

    #[test]
    fn libraries_may_not_use_apps() {
        let v = check(&[c("drafftink-ui", &[("drafftink", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 6, to: 8, .. }));
    }

    #[test]
    fn xtask_is_standalone() {
        let v = check(&[c("xtask", &[("drafftink-geom", Normal, true)])]);
        assert!(matches!(only(&v), Violation::ToolEdge { .. }));
        let v = check(&[c("drafftink-cli", &[("xtask", Dev, true)])]);
        assert!(matches!(only(&v), Violation::ToolEdge { .. }));
    }

    #[test]
    fn vendored_crates_are_leaves_that_any_library_may_use() {
        // Typeset (L1) uses ReX; ReX uses its table crate; neither reaches into our crates.
        let ok = [
            c("drafftink-typeset", &[("drafftink-rex", Normal, true)]),
            c("drafftink-rex", &[("drafftink-unicode-math", Normal, true), ("serde", Normal, false)]),
            c("drafftink-unicode-math", &[("regex", Dev, false)]),
        ];
        let v = check(&ok);
        assert!(v.iter().all(|v| !matches!(v, Violation::VendoredEdge { .. } | Violation::Edge { .. })), "{v:?}");
        let v = check(&[c("drafftink-rex", &[("drafftink-geom", Normal, true)])]);
        assert!(matches!(only(&v), Violation::VendoredEdge { .. }));
    }

    #[test]
    fn metadata_parsing() {
        let meta: Value = serde_json::from_str(
            r#"{"packages":[
                {"name":"drafftink-doc","dependencies":[
                    {"name":"drafftink-geom","kind":null,"path":"/x/crates/geom"},
                    {"name":"serde","kind":null},
                    {"name":"proptest","kind":"dev"}]},
                {"name":"drafftink-geom","dependencies":[]}
            ]}"#,
        )
        .unwrap();
        let g = from_metadata(&meta).unwrap();
        assert_eq!(g.len(), 2);
        let doc = g.iter().find(|c| c.name == "drafftink-doc").unwrap();
        assert!(doc.deps[0].workspace && !doc.deps[1].workspace);
        assert_eq!(doc.deps[2].kind, Dev);
        assert!(check(&g).is_empty());
    }

    #[test]
    fn metadata_without_packages_is_an_error() {
        assert!(from_metadata(&serde_json::json!({})).is_err());
    }
}
