//! `cargo xtask publish`: the dry run before a release to crates.io. It never publishes.
//!
//! What it does, in this order:
//!
//! 1. Reads `cargo metadata` and lists the crates that are published (every package without `publish = false`),
//!    in publish order: a crate comes after the crates it depends on.
//! 2. Checks what crates.io needs and `cargo package` does not insist on: the metadata of each crate, a version next to
//!    every internal path dependency, no Git dependency, and no hand-drawn font file in any package (the fonts are by
//!    Shara Weber, are not covered by the AGPL and stay out of crates.io, `docs/publishing.md`).
//! 3. Runs `cargo publish --workspace --dry-run`, which packages every crate and builds it from its package, with the
//!    unpublished siblings of the workspace taking the place of the registry.
//! 4. Does the same for the placeholder `deprecated/drafftink-core`, which is outside the workspace.
//! 5. Prints the size of every package (crates.io accepts at most 10 MiB) and the commands to run by hand.
//!
//! The dry run is the only mode. [`publish_command`] always passes `--dry-run`, and an argument that asks for a real
//! publish is refused. The release itself is done by the person who owns the crates.io account (`docs/publishing.md`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::{cargo, run};

const USAGE: &str = "\
usage: cargo xtask publish [--dry-run] [--allow-dirty] [--no-verify]

  Lists the crates in publish order, checks them, runs `cargo publish --workspace --dry-run` and prints the package
  sizes. It never publishes: a dry run is the only mode (--dry-run is accepted and changes nothing).
  --allow-dirty   accept uncommitted changes (the real publish needs a clean tree)
  --no-verify     package only; do not build each crate from its package (quick check of the file lists and sizes)";

/// The limit of crates.io for one `.crate` file.
const MAX_CRATE_BYTES: u64 = 10 * 1024 * 1024;

/// A crate with at most this many compressed bytes is not worth a remark.
const NOTABLE_CRATE_BYTES: u64 = 5 * 1024 * 1024;

/// Most keywords crates.io takes.
const MAX_KEYWORDS: usize = 5;

/// Words that must not appear in the name of a file inside any package: the hand-drawn fonts.
const FORBIDDEN_FILE_WORDS: [&str; 2] = ["gelpen", "vanilla"];

/// The placeholder for the old crate name, outside the workspace.
const DEPRECATED_CORE: &str = "deprecated/drafftink-core";

/// What the command line asked for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    allow_dirty: bool,
    no_verify: bool,
}

/// Reads the arguments. The only mode is the dry run, so an argument that would publish is an error.
fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    for arg in args {
        match arg.as_str() {
            "--dry-run" => {}
            "--allow-dirty" => options.allow_dirty = true,
            "--no-verify" => options.no_verify = true,
            "-h" | "--help" => return Err(USAGE.to_owned()),
            "--execute" | "--publish" | "--no-dry-run" | "--real" | "--yes" | "--token" => {
                return Err(format!(
                    "`{arg}`: cargo xtask publish never publishes. Run `cargo publish --workspace` yourself, \
                     as docs/publishing.md describes.\n\n{USAGE}"
                ));
            }
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    Ok(options)
}

/// A published crate, as far as the dry run needs to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Package {
    name: String,
    version: String,
    manifest_dir: PathBuf,
    /// Names of the published crates this one needs from the registry (normal and build dependencies).
    needs: Vec<String>,
    problems: Vec<String>,
}

/// Packages of `cargo metadata --no-deps` that are published, with the problems found in their manifests.
fn published(meta: &Value) -> Result<Vec<Package>, String> {
    let packages = meta["packages"].as_array().ok_or("cargo metadata: no packages array")?;
    let is_published = |p: &Value| p["publish"].as_array().is_none_or(|registries| !registries.is_empty());
    let names: Vec<&str> = packages.iter().filter(|p| is_published(p)).filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in packages.iter().filter(|p| is_published(p)) {
        let text = |key: &str| p[key].as_str().filter(|s| !s.trim().is_empty());
        let name = text("name").ok_or("cargo metadata: package without a name")?.to_owned();
        let manifest_dir =
            Path::new(text("manifest_path").unwrap_or_default()).parent().unwrap_or(Path::new("")).to_owned();
        let mut problems = Vec::new();
        for (key, what) in [("description", "a description"), ("license", "a license"), ("repository", "a repository")]
        {
            if text(key).is_none() {
                problems.push(format!("{name}: needs {what}"));
            }
        }
        if text("homepage").is_none() {
            problems.push(format!("{name}: needs a homepage"));
        }
        match text("readme") {
            None => problems.push(format!("{name}: needs a readme")),
            Some(readme) if !manifest_dir.join(readme).is_file() => {
                problems.push(format!("{name}: the readme `{readme}` does not exist"));
            }
            Some(_) => {}
        }
        let count = |key: &str| p[key].as_array().map_or(0, Vec::len);
        if !(1..=MAX_KEYWORDS).contains(&count("keywords")) {
            problems.push(format!("{name}: needs 1 to {MAX_KEYWORDS} keywords, has {}", count("keywords")));
        }
        if count("categories") == 0 {
            problems.push(format!("{name}: needs at least one category"));
        }
        let mut needs = Vec::new();
        for dep in p["dependencies"].as_array().into_iter().flatten() {
            let dep_name = dep["name"].as_str().unwrap_or_default();
            let kind = dep["kind"].as_str();
            if kind == Some("dev") {
                // Cargo strips a dev-dependency without a version from the package, so a path-only one is fine.
                continue;
            }
            let source = dep["source"].as_str().unwrap_or_default();
            if source.starts_with("git+") {
                problems.push(format!("{name}: depends on `{dep_name}` from Git; crates.io accepts no Git dependency"));
            }
            if dep["path"].is_string() {
                if dep["req"].as_str().is_none_or(|req| req == "*") {
                    problems.push(format!("{name}: the path dependency `{dep_name}` needs a `version`"));
                }
                if names.contains(&dep_name) {
                    needs.push(dep_name.to_owned());
                } else {
                    problems.push(format!("{name}: depends on `{dep_name}`, which is not published (publish = false)"));
                }
            }
        }
        needs.sort();
        needs.dedup();
        let version = text("version").unwrap_or_default().to_owned();
        out.push(Package { name, version, manifest_dir, needs, problems });
    }
    Ok(out)
}

/// The publish order: every crate after the crates it needs, ties broken by name so the list is stable.
fn publish_order(packages: &[Package]) -> Result<Vec<&Package>, String> {
    let mut done: Vec<&Package> = Vec::new();
    let mut waiting: Vec<&Package> = packages.iter().collect();
    waiting.sort_by(|a, b| a.name.cmp(&b.name));
    while !waiting.is_empty() {
        let ready = waiting.iter().position(|p| p.needs.iter().all(|n| done.iter().any(|d| &d.name == n)));
        let Some(index) = ready else {
            let stuck: Vec<&str> = waiting.iter().map(|p| p.name.as_str()).collect();
            return Err(format!("the published crates depend on each other in a cycle: {}", stuck.join(", ")));
        };
        done.push(waiting.remove(index));
    }
    Ok(done)
}

/// The files of a package that no package may have: the hand-drawn fonts.
fn forbidden_files(listing: &str) -> Vec<&str> {
    listing
        .lines()
        .filter(|file| {
            let lower = file.to_ascii_lowercase();
            FORBIDDEN_FILE_WORDS.iter().any(|word| lower.contains(word))
        })
        .collect()
}

/// `cargo publish` for the whole workspace. `--dry-run` is not optional: this is the only place the command is built.
fn publish_command(root: &Path, options: &Options) -> Command {
    let mut command = cargo(root);
    command.args(["publish", "--workspace", "--dry-run", "--locked"]);
    if options.allow_dirty {
        command.arg("--allow-dirty");
    }
    if options.no_verify {
        command.arg("--no-verify");
    }
    command
}

/// `cargo publish` for the placeholder crate, which has a workspace of its own. Its package goes to the target
/// directory of the workspace, next to the others.
fn publish_deprecated_command(root: &Path, options: &Options, target_dir: &Path) -> Command {
    let mut command = cargo(root);
    command.args(["publish", "--dry-run", "--manifest-path"]).arg(root.join(DEPRECATED_CORE).join("Cargo.toml"));
    command.arg("--target-dir").arg(target_dir);
    if options.allow_dirty {
        command.arg("--allow-dirty");
    }
    if options.no_verify {
        command.arg("--no-verify");
    }
    command
}

/// The output of a command that must succeed.
fn capture(command: &mut Command) -> Result<String, String> {
    let output = command.output().map_err(|e| format!("cannot run {:?}: {e}", command.get_program()))?;
    if !output.status.success() {
        return Err(format!(
            "{:?} {:?} failed with {}:\n{}",
            command.get_program(),
            command.get_args().collect::<Vec<_>>(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn metadata(root: &Path, manifest: Option<&Path>) -> Result<Value, String> {
    let mut command = cargo(root);
    command.args(["metadata", "--format-version", "1", "--no-deps"]);
    if let Some(manifest) = manifest {
        command.arg("--manifest-path").arg(manifest);
    }
    serde_json::from_str(&capture(&mut command)?).map_err(|e| format!("cargo metadata output: {e}"))
}

/// What `cargo package --list` says about a crate.
fn package_files(root: &Path, krate: &Package, manifest: Option<&Path>) -> Result<String, String> {
    let mut command = cargo(root);
    command.args(["package", "--list", "--allow-dirty", "--offline"]);
    match manifest {
        Some(manifest) => command.arg("--manifest-path").arg(manifest),
        None => command.args(["-p", &krate.name]),
    };
    capture(&mut command)
}

/// One row of the size table.
struct Size {
    name: String,
    version: String,
    files: usize,
    bytes: Option<u64>,
}

fn print_sizes(sizes: &[Size]) -> Result<(), String> {
    println!("\n{:<24} {:<8} {:>6} {:>14}", "package", "version", "files", "bytes (.crate)");
    let mut too_big = Vec::new();
    for size in sizes {
        let shown = size.bytes.map_or_else(|| "not packaged".to_owned(), |b| b.to_string());
        let note = match size.bytes {
            Some(b) if b > MAX_CRATE_BYTES => {
                too_big.push(size.name.as_str());
                "  OVER THE 10 MiB LIMIT"
            }
            Some(b) if b > NOTABLE_CRATE_BYTES => "  over half the limit",
            _ => "",
        };
        println!("{:<24} {:<8} {:>6} {:>14}{note}", size.name, size.version, size.files, shown);
    }
    let total: u64 = sizes.iter().filter_map(|s| s.bytes).sum();
    println!("{:<24} {:<8} {:>6} {:>14}", "total", "", sizes.iter().map(|s| s.files).sum::<usize>(), total);
    if too_big.is_empty() { Ok(()) } else { Err(format!("over the 10 MiB limit of crates.io: {}", too_big.join(", "))) }
}

/// Where a dry run leaves `<name>-<version>.crate`: in `<target>/package` for a workspace run, in its `tmp-crate`
/// folder for a single package.
fn crate_files(target_dir: &Path, name: &str, version: &str) -> [PathBuf; 2] {
    let package = target_dir.join("package");
    let file = format!("{name}-{version}.crate");
    [package.join(&file), package.join("tmp-crate").join(file)]
}

/// The size of the package the dry run has just written.
fn crate_bytes(target_dir: &Path, name: &str, version: &str) -> Option<u64> {
    crate_files(target_dir, name, version).iter().find_map(|path| fs::metadata(path).ok()).map(|m| m.len())
}

/// Removes the packages of an earlier run, so that a size is always from this run.
fn forget_packages(target_dir: &Path, packages: &[Size]) {
    for size in packages {
        for path in crate_files(target_dir, &size.name, &size.version) {
            // A file that is not there is the normal case.
            let _ = fs::remove_file(path);
        }
    }
}

fn target_directory(meta: &Value) -> PathBuf {
    meta["target_directory"].as_str().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("target"))
}

/// The commands the owner of the crates.io account runs by hand, in order.
fn manual_steps(order: &[&Package]) -> String {
    let mut text = String::from("\nTo publish (by hand, from a clean checkout of the release commit):\n\n");
    text.push_str("  cargo publish --workspace\n\n  or one by one, in this order:\n");
    for p in order {
        text.push_str(&format!("    cargo publish -p {}\n", p.name));
    }
    text.push_str(&format!(
        "\n  and the placeholder for the old name:\n    cargo publish --manifest-path {DEPRECATED_CORE}/Cargo.toml\n"
    ));
    text.push_str(
        "\nThen yank drafftink-core 0.1.0 (cargo yank --version 0.1.0 drafftink-core). See docs/publishing.md.\n",
    );
    text
}

/// Entry point of `cargo xtask publish`.
pub fn run_publish(root: &Path, args: &[String]) -> Result<(), String> {
    let options = parse_args(args)?;
    let meta = metadata(root, None)?;
    let packages = published(&meta)?;
    let order = publish_order(&packages)?;
    println!("publish order ({} crates):", order.len());
    for (i, p) in order.iter().enumerate() {
        println!("  {:>2}. {} {}", i + 1, p.name, p.version);
    }
    println!("      + {DEPRECATED_CORE} (outside the workspace)\n");

    let mut problems: Vec<String> = order.iter().flat_map(|p| p.problems.iter().cloned()).collect();
    let mut sizes = Vec::new();
    for p in &order {
        let listing = package_files(root, p, None)?;
        problems.extend(forbidden_files(&listing).into_iter().map(|f| format!("{}: the package contains {f}", p.name)));
        sizes.push(Size {
            name: p.name.clone(),
            version: p.version.clone(),
            files: listing.lines().count(),
            bytes: None,
        });
    }
    let core_meta = metadata(root, Some(&root.join(DEPRECATED_CORE).join("Cargo.toml")))?;
    let core = published(&core_meta)?;
    let core = core.first().ok_or("deprecated/drafftink-core: not a published package")?;
    problems.extend(core.problems.iter().cloned());
    let listing = package_files(root, core, Some(&root.join(DEPRECATED_CORE).join("Cargo.toml")))?;
    sizes.push(Size {
        name: core.name.clone(),
        version: core.version.clone(),
        files: listing.lines().count(),
        bytes: None,
    });
    if !problems.is_empty() {
        for problem in &problems {
            eprintln!("  {problem}");
        }
        return Err(format!("{} problem(s) before packaging; see docs/publishing.md", problems.len()));
    }
    println!(
        "checks: metadata, versions of path dependencies, no Git dependency, no hand-drawn font in any package: ok\n"
    );

    let target_dir = target_directory(&meta);
    forget_packages(&target_dir, &sizes);
    run(&mut publish_command(root, &options))?;
    run(&mut publish_deprecated_command(root, &options, &target_dir))?;

    let sizes: Vec<Size> =
        sizes.into_iter().map(|s| Size { bytes: crate_bytes(&target_dir, &s.name, &s.version), ..s }).collect();
    print_sizes(&sizes)?;
    println!("{}", manual_steps(&order));
    Ok(())
}

/// A helper for the tests: the packages by name.
#[cfg(test)]
fn by_name(packages: &[Package]) -> std::collections::BTreeMap<&str, &Package> {
    packages.iter().map(|p| (p.name.as_str(), p)).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    fn package(name: &str, needs: &[&str]) -> Package {
        Package {
            name: name.into(),
            version: "0.2.0".into(),
            manifest_dir: PathBuf::new(),
            needs: needs.iter().map(|s| (*s).to_owned()).collect(),
            problems: Vec::new(),
        }
    }

    #[test]
    fn the_dry_run_is_the_only_mode() {
        assert_eq!(parse_args(&[]), Ok(Options::default()));
        assert_eq!(parse_args(&strings(&["--dry-run"])), Ok(Options::default()));
        assert_eq!(
            parse_args(&strings(&["--dry-run", "--allow-dirty", "--no-verify"])),
            Ok(Options { allow_dirty: true, no_verify: true })
        );
        for real in ["--execute", "--publish", "--no-dry-run", "--real", "--yes", "--token"] {
            let error = parse_args(&strings(&[real])).unwrap_err();
            assert!(error.contains("never publishes"), "{real}: {error}");
        }
        assert!(parse_args(&strings(&["--bogus"])).is_err());
    }

    #[test]
    fn every_cargo_publish_command_has_dry_run() {
        for options in [Options::default(), Options { allow_dirty: true, no_verify: true }] {
            let root = Path::new("/repo");
            let commands = [
                publish_command(root, &options),
                publish_deprecated_command(root, &options, Path::new("/repo/target")),
            ];
            for command in commands {
                let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
                assert_eq!(args.first().map(String::as_str), Some("publish"), "{args:?}");
                assert!(args.iter().any(|a| a == "--dry-run"), "{args:?}");
                assert!(args.iter().all(|a| a != "--token"), "{args:?}");
            }
        }
    }

    #[test]
    fn dependencies_come_first_and_ties_are_alphabetical() {
        let packages = [
            package("c-app", &["b-lib", "a-lib"]),
            package("b-lib", &["a-lib"]),
            package("a-lib", &[]),
            package("a-other", &[]),
        ];
        let order: Vec<&str> = publish_order(&packages).unwrap().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(order, ["a-lib", "a-other", "b-lib", "c-app"]);
    }

    #[test]
    fn a_cycle_is_an_error() {
        let packages = [package("a", &["b"]), package("b", &["a"])];
        assert!(publish_order(&packages).unwrap_err().contains("cycle"));
    }

    #[test]
    fn hand_drawn_font_files_are_found_in_a_listing() {
        let listing = "Cargo.toml\nassets/fonts/NotoSans-Regular.ttf\nassets/fonts/GelPenLight.ttf\nassets/fonts/VanillaExtract.ttf\n";
        assert_eq!(forbidden_files(listing), ["assets/fonts/GelPenLight.ttf", "assets/fonts/VanillaExtract.ttf"]);
        assert!(forbidden_files("src/lib.rs\nassets/fonts/IBMPlexSans-Variable.ttf\n").is_empty());
    }

    fn metadata_of(packages: Value) -> Value {
        json!({ "packages": packages })
    }

    fn complete(name: &str, deps: Value) -> Value {
        json!({
            "name": name, "version": "0.2.0", "manifest_path": "/nowhere/Cargo.toml", "publish": null,
            "description": "d", "license": "AGPL-3.0-only", "repository": "r", "homepage": "h", "readme": "README.md",
            "keywords": ["a"], "categories": ["graphics"], "dependencies": deps,
        })
    }

    /// A package with `publish = false`.
    fn unpublished(name: &str) -> Value {
        let mut package = complete(name, json!([]));
        package["publish"] = json!([]);
        package
    }

    #[test]
    fn unpublished_packages_are_left_out_and_path_dependencies_need_a_version() {
        let meta = metadata_of(json!([
            complete("lib", json!([])),
            complete(
                "app",
                json!([
                    { "name": "lib", "kind": null, "path": "/x/lib", "req": "*", "source": null },
                    { "name": "testkit", "kind": "dev", "path": "/x/testkit", "req": "*", "source": null },
                ])
            ),
            unpublished("tool"),
        ]));
        let packages = published(&meta).unwrap();
        let by = by_name(&packages);
        assert_eq!(by.len(), 2, "the tool has publish = false");
        assert_eq!(by["app"].needs, ["lib"]);
        assert!(by["app"].problems.iter().any(|p| p.contains("`lib` needs a `version`")), "{:?}", by["app"].problems);
        assert!(by["app"].problems.iter().all(|p| !p.contains("testkit")), "a path-only dev-dependency is stripped");
    }

    #[test]
    fn git_dependencies_and_dependencies_on_unpublished_crates_are_problems() {
        let meta = metadata_of(json!([
            complete(
                "app",
                json!([
                    { "name": "rex", "kind": null, "req": "*", "source": "git+https://example.com/rex" },
                    { "name": "private", "kind": null, "path": "/x/private", "req": "^0.2.0", "source": null },
                ])
            ),
            unpublished("private"),
        ]));
        let problems = &published(&meta).unwrap()[0].problems;
        assert!(problems.iter().any(|p| p.contains("Git")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("publish = false")), "{problems:?}");
    }

    #[test]
    fn missing_metadata_is_listed() {
        let mut bare = complete("bare", json!([]));
        for key in ["description", "homepage", "readme"] {
            bare[key] = Value::Null;
        }
        bare["keywords"] = json!(["a", "b", "c", "d", "e", "f"]);
        bare["categories"] = json!([]);
        let problems = &published(&metadata_of(json!([bare]))).unwrap()[0].problems;
        for word in ["a description", "a homepage", "a readme", "1 to 5 keywords", "at least one category"] {
            assert!(problems.iter().any(|p| p.contains(word)), "{word}: {problems:?}");
        }
    }
}
