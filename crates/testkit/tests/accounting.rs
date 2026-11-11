#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The harness accounting test (docs/plan.md §8, "harness discipline"). Run by `cargo xtask harness`.
//!
//! The harness has a method only for what a command cannot do, so every method must earn its place:
//!
//! 1. every public method of `Harness` (one that takes `self`) has a doc line `Reason: ...`;
//! 2. the scenario dispatcher (`HARNESS_METHODS`) and the declared methods are the same set;
//! 3. every declared method is called by at least one scenario file or one Rust integration test
//!    (a method nobody calls is deleted, not kept);
//! 4. no scenario step and no Rust integration test calls a method that is not declared.
//!
//! And for scenarios:
//!
//! 5. every command family in the registry has at least one scenario file in `tests/scenarios/<family>/`
//!    (unless `tests/scenarios/pending-families.txt` lists it; that list only shrinks);
//! 6. every command a scenario names is registered, or is listed in that scenario's `requires.commands`;
//!    a `requires` entry that is registered now is stale and must be removed so the scenario runs; and an entry
//!    that is not registered must be a real planned command (a row of docs/commands.md).
//!
//! It prints a report (`--nocapture`): the waiting scenarios and the commands no scenario runs yet.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use drafftink_engine::Registry;
use drafftink_testkit::scenario::{HARNESS_METHODS, Scenario};
use syn::visit::Visit;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ---------------------------------------------------------------------------------------------
// Reading the harness declaration
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Declared {
    name: String,
    doc: String,
}

fn doc_of(attrs: &[syn::Attribute]) -> String {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_harness_type(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(p) => p.path.segments.last().is_some_and(|s| s.ident == "Harness"),
        syn::Type::Reference(r) => is_harness_type(&r.elem),
        _ => false,
    }
}

/// The public methods (with a `self` receiver) of every inherent `impl Harness` in `src`.
fn declared_methods(src: &str) -> Result<Vec<Declared>, String> {
    let file = syn::parse_file(src).map_err(|e| format!("cannot parse the harness source: {e}"))?;
    let mut out = Vec::new();
    for item in &file.items {
        let syn::Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() || !is_harness_type(&imp.self_ty) {
            continue;
        }
        for it in &imp.items {
            let syn::ImplItem::Fn(f) = it else { continue };
            if matches!(f.vis, syn::Visibility::Public(_)) && f.sig.receiver().is_some() {
                out.push(Declared { name: f.sig.ident.to_string(), doc: doc_of(&f.attrs) });
            }
        }
    }
    Ok(out)
}

fn has_reason(doc: &str) -> bool {
    doc.find("Reason:").is_some_and(|at| doc[at + "Reason:".len()..].trim().chars().count() >= 40)
}

// ---------------------------------------------------------------------------------------------
// Reading Rust integration tests
// ---------------------------------------------------------------------------------------------

/// Is `e` (a `let` initialiser) a harness built by `Harness::...`, possibly through `?`, `.unwrap()` and friends?
fn is_harness_ctor(mut e: &syn::Expr) -> bool {
    loop {
        match e {
            syn::Expr::Try(t) => e = &t.expr,
            syn::Expr::MethodCall(m) => e = &m.receiver,
            syn::Expr::Paren(p) => e = &p.expr,
            syn::Expr::Call(c) => {
                return matches!(&*c.func, syn::Expr::Path(p)
                    if p.path.segments.len() >= 2 && p.path.segments.iter().rev().nth(1).is_some_and(|s| s.ident == "Harness"));
            }
            _ => return false,
        }
    }
}

#[derive(Default)]
struct Calls {
    vars: BTreeSet<String>,
    found: BTreeSet<String>,
}

impl Calls {
    fn scoped(&mut self, f: impl FnOnce(&mut Self)) {
        let saved = std::mem::take(&mut self.vars);
        f(self);
        self.vars = saved;
    }

    fn add_params(&mut self, sig: &syn::Signature) {
        for arg in &sig.inputs {
            if let syn::FnArg::Typed(t) = arg
                && is_harness_type(&t.ty)
                && let Some(n) = pat_ident(&t.pat)
            {
                self.vars.insert(n);
            }
        }
    }
}

fn pat_ident(p: &syn::Pat) -> Option<String> {
    match p {
        syn::Pat::Ident(i) => Some(i.ident.to_string()),
        syn::Pat::Type(t) => pat_ident(&t.pat),
        _ => None,
    }
}

impl<'ast> Visit<'ast> for Calls {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.scoped(|me| {
            me.add_params(&f.sig);
            syn::visit::visit_item_fn(me, f);
        });
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.scoped(|me| {
            me.add_params(&f.sig);
            syn::visit::visit_impl_item_fn(me, f);
        });
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        if let (Some(name), Some(init)) = (pat_ident(&l.pat), &l.init)
            && is_harness_ctor(&init.expr)
        {
            self.vars.insert(name);
        }
        syn::visit::visit_local(self, l);
    }

    fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
        if let syn::Expr::Path(p) = &*m.receiver
            && let Some(id) = p.path.get_ident()
            && self.vars.contains(&id.to_string())
        {
            self.found.insert(m.method.to_string());
        }
        syn::visit::visit_expr_method_call(self, m);
    }
}

/// Method names called on a `Harness` value in `src`: variables built with `Harness::...` and parameters typed
/// `Harness`, `&Harness` or `&mut Harness`. Field accesses (`h.session.execute(..)`) are not harness methods.
fn rust_calls(src: &str) -> Result<BTreeSet<String>, String> {
    let file = syn::parse_file(src).map_err(|e| e.to_string())?;
    let mut calls = Calls::default();
    calls.visit_file(&file);
    Ok(calls.found)
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every Rust integration test of the workspace: `crates/*/tests`, `apps/*/tests` and `tests/` (not fixtures).
fn integration_test_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for group in ["crates", "apps"] {
        let Ok(rd) = fs::read_dir(root().join(group)) else { continue };
        let mut dirs: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            rust_files(&d.join("tests"), &mut files);
        }
    }
    let top = root().join("tests");
    if let Ok(rd) = fs::read_dir(&top) {
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() && name != "fixtures" && name != "scenarios" {
                rust_files(&p, &mut files);
            } else if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    files
}

// ---------------------------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------------------------

type Where = BTreeMap<String, BTreeSet<String>>;

struct Evidence<'a> {
    /// Method name -> the scenario files that call it.
    scenario_calls: &'a Where,
    /// Method name -> the Rust test files that call it.
    rust_calls: &'a Where,
}

/// Rules 1 to 4. Pure, so the tests below can feed it broken inputs.
fn check_methods(declared: &[Declared], dispatched: &[&str], ev: &Evidence<'_>) -> Vec<String> {
    let mut problems = Vec::new();
    let names: BTreeSet<&str> = declared.iter().map(|d| d.name.as_str()).collect();
    for d in declared {
        if !has_reason(&d.doc) {
            problems.push(format!(
                "Harness::{} has no `Reason:` doc line (at least a sentence). A harness method exists only for what a command cannot do; say what that is.",
                d.name
            ));
        }
        if !dispatched.contains(&d.name.as_str()) {
            problems.push(format!(
                "Harness::{} is not in HARNESS_METHODS (crates/testkit/src/scenario.rs), so no scenario can call it. Add it to `scenario_methods!`.",
                d.name
            ));
        }
        if !ev.scenario_calls.contains_key(&d.name) && !ev.rust_calls.contains_key(&d.name) {
            problems.push(format!(
                "Harness::{} is declared but nothing calls it: no scenario step and no integration test. Delete it, or add the scenario that needs it.",
                d.name
            ));
        }
    }
    for m in dispatched {
        if !names.contains(m) {
            problems.push(format!("HARNESS_METHODS lists `{m}`, but Harness has no public method of that name."));
        }
    }
    for (kind, calls) in [("scenario", ev.scenario_calls), ("Rust test", ev.rust_calls)] {
        for (name, whence) in calls {
            if !names.contains(name.as_str()) {
                problems.push(format!(
                    "{kind} calls harness method `{name}`, which is not declared on Harness (in {}). Declare it with a `Reason:`, or use a command.",
                    whence.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
    }
    problems
}

struct Loaded {
    /// `family/file.json` -> parsed.
    scenarios: BTreeMap<String, Scenario>,
    /// Files that do not parse, with the error.
    broken: Vec<String>,
}

fn load_scenarios(dir: &Path) -> Loaded {
    let mut out = Loaded { scenarios: BTreeMap::new(), broken: Vec::new() };
    let Ok(rd) = fs::read_dir(dir) else { return out };
    let mut families: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    families.sort();
    for fam in families {
        let fname = fam.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let Ok(rd) = fs::read_dir(&fam) else { continue };
        let mut files: Vec<_> = rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for f in files {
            let key =
                format!("{fname}/{}", f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
            match fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|t| Scenario::parse(&t)) {
                Ok(sc) => {
                    out.scenarios.insert(key, sc);
                }
                Err(e) => out.broken.push(format!("{key}: {e}")),
            }
        }
    }
    out
}

/// The command ids that start a row of a table in docs/commands.md.
fn planned_commands(docs: &str) -> BTreeSet<String> {
    docs.lines()
        .filter_map(|l| l.strip_prefix("| `"))
        .filter_map(|rest| rest.split_once("` |"))
        .map(|(id, _)| id.to_owned())
        .collect()
}

/// Rules 5 and 6. `registered` are the registry's command ids, `families` its families.
fn check_scenarios(
    scenarios: &BTreeMap<String, Scenario>,
    registered: &BTreeSet<String>,
    families: &[&str],
    pending_families: &[String],
    planned: &BTreeSet<String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let with_scenarios: BTreeSet<&str> = scenarios.keys().filter_map(|k| k.split_once('/')).map(|(f, _)| f).collect();
    for fam in families {
        let pending = pending_families.iter().any(|p| p == fam);
        match (with_scenarios.contains(fam), pending) {
            (false, false) => problems.push(format!(
                "family `{fam}` has no scenario. Add tests/scenarios/{fam}/<name>.json: the ok path, one refusal, and undo (docs/testing.md)."
            )),
            (true, true) => problems.push(format!(
                "tests/scenarios/pending-families.txt lists `{fam}`, but it has scenarios now. Remove the line (the list only shrinks)."
            )),
            _ => {}
        }
    }
    for p in pending_families {
        if !families.contains(&p.as_str()) {
            problems.push(format!(
                "tests/scenarios/pending-families.txt lists `{p}`, which is not a registered family. Remove the line."
            ));
        }
    }
    for (file, sc) in scenarios {
        for id in sc.commands() {
            if !registered.contains(&id) && !sc.requires.commands.contains(&id) {
                problems.push(format!(
                    "{file} names command `{id}`, which is not registered. Fix the name, or add it to `requires.commands` if the command is planned."
                ));
            }
        }
        for id in &sc.requires.commands {
            if registered.contains(id) {
                problems.push(format!("{file} waits for `{id}`, which is registered now. Remove it from `requires.commands` so the scenario runs."));
            } else if !planned.contains(id) {
                problems.push(format!(
                    "{file} waits for `{id}`, which is not a command in docs/commands.md. Fix the name."
                ));
            }
        }
    }
    problems
}

fn pending_families(root: &Path) -> Vec<String> {
    let text = fs::read_to_string(root.join("tests/scenarios/pending-families.txt")).unwrap_or_default();
    text.lines().map(|l| l.split('#').next().unwrap_or_default().trim().to_owned()).filter(|l| !l.is_empty()).collect()
}

// ---------------------------------------------------------------------------------------------
// The workspace
// ---------------------------------------------------------------------------------------------

#[test]
fn the_harness_and_the_scenarios_account_for_each_other() {
    let root = root();
    let harness_src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/harness.rs")).unwrap();
    let declared = declared_methods(&harness_src).unwrap();
    assert!(
        !declared.is_empty(),
        "found no public Harness methods: did the accounting parser lose track of src/harness.rs?"
    );

    let loaded = load_scenarios(&root.join("tests/scenarios"));
    assert!(loaded.broken.is_empty(), "scenario files that do not parse:\n  {}", loaded.broken.join("\n  "));

    let mut scenario_calls = Where::new();
    for (file, sc) in &loaded.scenarios {
        for m in sc.harness_calls() {
            scenario_calls.entry(m).or_default().insert(file.clone());
        }
    }
    let mut rust = Where::new();
    let rust_paths = integration_test_files();
    let mut reads = 0;
    for f in &rust_paths {
        let src = fs::read_to_string(f).unwrap();
        if !src.contains("drafftink_testkit") {
            continue; // Other crates have harnesses of their own (collab); only the testkit's counts.
        }
        reads += 1;
        let shown = f.strip_prefix(&root).unwrap_or(f).display().to_string();
        let found = rust_calls(&src).unwrap_or_else(|e| panic!("cannot parse {shown}: {e}"));
        for m in found {
            rust.entry(m).or_default().insert(shown.clone());
        }
    }

    let registry = Registry::global();
    let registered: BTreeSet<String> = registry.all().iter().map(|s| s.id.to_owned()).collect();
    let docs = fs::read_to_string(root.join("docs/commands.md")).unwrap_or_default();

    let mut problems =
        check_methods(&declared, HARNESS_METHODS, &Evidence { scenario_calls: &scenario_calls, rust_calls: &rust });
    problems.extend(check_scenarios(
        &loaded.scenarios,
        &registered,
        &registry.families(),
        &pending_families(&root),
        &planned_commands(&docs),
    ));

    // The report.
    let used: BTreeSet<String> = loaded.scenarios.values().flat_map(Scenario::commands).collect();
    let waiting: Vec<String> = loaded
        .scenarios
        .iter()
        .filter(|(_, sc)| !sc.requires.commands.is_empty())
        .map(|(f, sc)| format!("{f} (waits for {})", sc.requires.commands.join(", ")))
        .collect();
    let unused: Vec<&str> = registered.iter().filter(|c| !used.contains(*c)).map(String::as_str).collect();
    println!(
        "harness: {} methods, {} scenario files, {} of {} integration test files use the testkit",
        declared.len(),
        loaded.scenarios.len(),
        reads,
        rust_paths.len()
    );
    println!(
        "scenarios waiting for commands: {}",
        if waiting.is_empty() { "none".to_owned() } else { waiting.join("; ") }
    );
    println!(
        "commands exercised by scenarios: {} of {}{}",
        registered.len() - unused.len(),
        registered.len(),
        if unused.is_empty() { String::new() } else { format!("; not yet: {}", unused.join(", ")) }
    );

    assert!(problems.is_empty(), "harness accounting failed:\n  - {}", problems.join("\n  - "));
}

// ---------------------------------------------------------------------------------------------
// The rules themselves: broken inputs must fail
// ---------------------------------------------------------------------------------------------

const HARNESS_SRC: &str = r#"
    pub struct Harness;
    impl Harness {
        /// A constructor needs no reason.
        pub fn new() -> Self { Harness }
        /// Does a thing.
        ///
        /// Reason: the clock is a port and a command cannot move the fake one, so tests need this.
        pub fn good(&mut self) {}
        /// Does a thing without saying why.
        pub fn lazy(&self) {}
        /// Reason: short.
        pub fn terse(&self) {}
        fn private_helper(&self) {}
    }
    impl Other { pub fn not_ours(&self) {} }
    impl Clone for Harness { fn clone(&self) -> Self { Harness } }
"#;

fn map(entries: &[(&str, &str)]) -> Where {
    let mut m = Where::new();
    for (k, v) in entries {
        m.entry((*k).to_owned()).or_default().insert((*v).to_owned());
    }
    m
}

#[test]
fn only_public_methods_with_a_receiver_of_inherent_harness_impls_are_declared() {
    let names: Vec<String> = declared_methods(HARNESS_SRC).unwrap().into_iter().map(|d| d.name).collect();
    assert_eq!(names, ["good", "lazy", "terse"]);
}

#[test]
fn a_method_without_a_real_reason_is_reported() {
    let declared = declared_methods(HARNESS_SRC).unwrap();
    let sc = map(&[("good", "a.json"), ("lazy", "a.json"), ("terse", "a.json")]);
    let problems = check_methods(
        &declared,
        &["good", "lazy", "terse"],
        &Evidence { scenario_calls: &sc, rust_calls: &Where::new() },
    );
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("Harness::lazy") && p.contains("`Reason:`")));
    assert!(problems.iter().any(|p| p.contains("Harness::terse")));
}

#[test]
fn a_method_nobody_calls_is_reported() {
    let declared = declared_methods(HARNESS_SRC).unwrap();
    let problems =
        check_methods(&declared[..1], &["good"], &Evidence { scenario_calls: &map(&[]), rust_calls: &map(&[]) });
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("Harness::good is declared but nothing calls it"), "{problems:?}");
}

#[test]
fn a_scenario_call_or_a_rust_test_call_counts_as_use() {
    let declared = declared_methods(HARNESS_SRC).unwrap();
    let from_rust = Evidence { scenario_calls: &map(&[]), rust_calls: &map(&[("good", "crates/x/tests/t.rs")]) };
    assert!(check_methods(&declared[..1], &["good"], &from_rust).is_empty());
    let from_scenario = Evidence { scenario_calls: &map(&[("good", "a/b.json")]), rust_calls: &map(&[]) };
    assert!(check_methods(&declared[..1], &["good"], &from_scenario).is_empty());
}

#[test]
fn a_call_to_an_undeclared_method_is_reported_for_scenarios_and_for_rust() {
    let declared = declared_methods(HARNESS_SRC).unwrap();
    let sc = map(&[("good", "a.json"), ("ghost", "b.json")]);
    let rs = map(&[("phantom", "crates/x/tests/t.rs")]);
    let problems = check_methods(&declared[..1], &["good"], &Evidence { scenario_calls: &sc, rust_calls: &rs });
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().any(|p| p.starts_with("scenario calls harness method `ghost`") && p.contains("b.json")));
    assert!(problems.iter().any(|p| p.starts_with("Rust test calls harness method `phantom`") && p.contains("t.rs")));
}

#[test]
fn the_dispatcher_and_the_declaration_must_agree() {
    let declared = declared_methods(HARNESS_SRC).unwrap();
    let sc = map(&[("good", "a.json")]);
    let ev = Evidence { scenario_calls: &sc, rust_calls: &map(&[]) };
    let problems = check_methods(&declared[..1], &["good", "extra"], &ev);
    assert!(problems.iter().any(|p| p.contains("lists `extra`")), "{problems:?}");
    let problems = check_methods(&declared[..1], &[], &ev);
    assert!(problems.iter().any(|p| p.contains("not in HARNESS_METHODS")), "{problems:?}");
}

#[test]
fn rust_calls_follow_variables_built_by_harness_constructors_and_typed_parameters() {
    let src = r#"
        fn a() {
            let mut h = Harness::new();
            h.stage_fixture("x");
            h.session.execute("view.get", json!({}));
            let other = Something::new();
            other.not_a_harness_method();
            let r = Harness::headless().unwrap();
            r.advance_clock(3);
            let s = drafftink_testkit::Harness::new()?;
            s.load_generated(1, 1);
        }
        fn b(h: &mut Harness, g: &Harness, n: u32) {
            h.pixel_probe(&args);
            g.load_generated(1, 1);
            n.pow(2);
        }
        fn c() {
            // `h` is not a harness here, although it is in `a`.
            let h = 5;
            h.pow(2);
        }
    "#;
    let found: Vec<String> = rust_calls(src).unwrap().into_iter().collect();
    assert_eq!(found, ["advance_clock", "load_generated", "pixel_probe", "stage_fixture"]);
}

fn scenario(text: &str) -> Scenario {
    Scenario::parse(text).unwrap()
}

#[test]
fn every_family_needs_a_scenario_unless_it_is_pending_and_a_stale_pending_line_fails() {
    let one =
        BTreeMap::from([("view/a.json".to_owned(), scenario(r#"{"about":"a","steps":[{"execute":"view.get"}]}"#))]);
    let reg: BTreeSet<String> = ["view.get".to_owned()].into();
    let none = BTreeSet::new();
    assert!(check_scenarios(&one, &reg, &["view"], &[], &none).is_empty());
    let p = check_scenarios(&one, &reg, &["view", "tab"], &[], &none);
    assert_eq!(p.len(), 1);
    assert!(p[0].contains("family `tab` has no scenario"), "{p:?}");
    assert!(check_scenarios(&one, &reg, &["view", "tab"], &["tab".to_owned()], &none).is_empty());
    let p = check_scenarios(&one, &reg, &["view"], &["view".to_owned()], &none);
    assert!(p[0].contains("has scenarios now"), "{p:?}");
    let p = check_scenarios(&one, &reg, &["view"], &["gone".to_owned()], &none);
    assert!(p[0].contains("not a registered family"), "{p:?}");
}

#[test]
fn commands_in_scenarios_must_exist_or_be_awaited_and_awaited_ones_must_be_real_and_not_yet_registered() {
    let reg: BTreeSet<String> = ["view.get".to_owned()].into();
    let planned: BTreeSet<String> = ["element.move".to_owned()].into();
    let mk = |text: &str| BTreeMap::from([("view/a.json".to_owned(), scenario(text))]);
    // A typo in a command name.
    let p = check_scenarios(&mk(r#"{"about":"a","steps":[{"execute":"view.gett"}]}"#), &reg, &["view"], &[], &planned);
    assert!(p.iter().any(|m| m.contains("`view.gett`") && m.contains("not registered")), "{p:?}");
    // Awaited and planned: fine.
    let ok = r#"{"about":"a","requires":{"commands":["element.move"]},"steps":[{"execute":"element.move"},{"execute":"view.get"}]}"#;
    assert!(check_scenarios(&mk(ok), &reg, &["view"], &[], &planned).is_empty());
    // Awaited but nobody ever planned it: a typo that would skip the scenario forever.
    let typo = r#"{"about":"a","requires":{"commands":["element.mvoe"]},"steps":[{"execute":"element.mvoe"},{"execute":"view.get"}]}"#;
    let p = check_scenarios(&mk(typo), &reg, &["view"], &[], &planned);
    assert!(p.iter().any(|m| m.contains("not a command in docs/commands.md")), "{p:?}");
    // Awaited but registered now: the scenario would be skipped for no reason.
    let stale = r#"{"about":"a","requires":{"commands":["view.get"]},"steps":[{"execute":"view.get"}]}"#;
    let p = check_scenarios(&mk(stale), &reg, &["view"], &[], &planned);
    assert!(p.iter().any(|m| m.contains("registered now")), "{p:?}");
}

#[test]
fn planned_commands_are_the_first_cell_of_the_command_tables() {
    let docs = "| `element.move` | Move | Gesture |\n| `view.pan` | Pan |\nText with `inline.code` | not a row\n| plain | row |\n";
    let ids: Vec<String> = planned_commands(docs).into_iter().collect();
    assert_eq!(ids, ["element.move", "view.pan"]);
}
