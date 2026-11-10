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
            Class::Tool => None,
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
    // L5
    ("drafftink-engine", Class::Layer(5)),
    ("drafftink-collab", Class::Layer(5)),
    // L6
    ("drafftink-gpu", Class::Layer(6)),
    ("drafftink-ui", Class::Layer(6)),
    ("drafftink-bridge", Class::Layer(6)),
    // L7
    ("drafftink-testkit", Class::Testkit),
    // apps
    ("drafftink", Class::App),
    ("drafftink-web", Class::App),
    ("drafftink-server", Class::App),
    ("drafftink-cli", Class::App),
    ("drafftink-mcp", Class::App),
    // tooling
    ("xtask", Class::Tool),
];

/// Apps (and other non-library packages) that also build for wasm32-unknown-unknown, on top of
/// every crate at L0 to L5.
pub const WASM_EXTRA: &[&str] = &["drafftink-web"];

/// A same-layer edge that is allowed, with the reason. Earlier name may be used by the later one.
/// Keep this list short: each entry is a hole in the layering.
pub struct SameLayerEdge {
    pub lower: &'static str,
    pub upper: &'static str,
    pub reason: &'static str,
}

pub const SAME_LAYER_EDGES: &[SameLayerEdge] = &[
    SameLayerEdge {
        lower: "drafftink-canvas",
        upper: "drafftink-doc",
        reason: "doc implements canvas's SceneStore trait with Loro",
    },
    SameLayerEdge {
        lower: "drafftink-engine",
        upper: "drafftink-collab",
        reason: "remote edits enter through the engine",
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

fn matches_pattern(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
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
            Violation::TestkitAsNormalDep { krate } => {
                write!(f, "{krate}: drafftink-testkit may only be a dev-dependency")
            }
            Violation::External { krate, dep, layer, min_layer, why } => {
                write!(f, "{krate} (L{layer}) depends on `{dep}`, allowed from L{min_layer}: {why}")
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
                check_external(&mut out, &c.name, layer, &d.name);
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

fn check_external(out: &mut Vec<Violation>, krate: &str, layer: u8, dep: &str) {
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
            let workspace = members.contains(&dep_name.as_str()) || d["path"].is_string();
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
        assert_eq!(layer("drafftink-collab"), Some(5));
        for n in ["gpu", "ui", "bridge"] {
            assert_eq!(layer(&format!("drafftink-{n}")), Some(6));
        }
        assert_eq!(layer("drafftink-testkit"), Some(7));
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
    fn allowed_same_layer_edges_only_go_one_way() {
        assert!(check(&[c("drafftink-doc", &[("drafftink-canvas", Normal, true)])]).is_empty());
        let v = check(&[c("drafftink-canvas", &[("drafftink-doc", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { from: 3, to: 3, .. }));
        assert!(check(&[c("drafftink-collab", &[("drafftink-engine", Normal, true)])]).is_empty());
        let v = check(&[c("drafftink-engine", &[("drafftink-collab", Normal, true)])]);
        assert!(matches!(only(&v), Violation::Edge { .. }));
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
        assert!(check(&[c("drafftink-render", &[("wgpu", Normal, false), ("wgpu-types", Normal, false)])]).is_empty());
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
        assert!(check(&[c("drafftink-canvas", &[("vello", Normal, false)])]).is_empty());
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
