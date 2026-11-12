//! `cargo xtask size`: the wasm size gate (`docs/performance.md`, "Wasm size").
//!
//! Builds the web app the way a release is built (`scripts/build-web.sh`: `release-wasm` profile, wasm-bindgen,
//! wasm-opt), measures the module and the font files that the page loads next to it, and checks the numbers against
//! the `size` section of `perf/budgets.json`. On a failure it prints the biggest items of the unstripped module
//! (`twiggy top`), so the commit that grew the file can be found.
//!
//! ```text
//! cargo xtask size                  build, measure, check
//! cargo xtask size --no-build       measure the existing dist/ (after scripts/build-web.sh)
//! cargo xtask size --wasm <file>    measure a finished wasm module (the one wasm-opt wrote); fonts come from dist/ if there
//! cargo xtask size --native         build the shipped binaries (profile `dist`) and check their sizes
//! cargo xtask size --native --no-build   measure the binaries that are already in target/dist
//! ```
//!
//! The build script measures the module and the fonts with the C compressors (`gzip -9`, `brotli -q 11`) and
//! writes `dist/sizes.json`; the gate reads that file. `--wasm` compresses in this process instead, which is
//! slow for the brotli part (minutes) unless the `brotli` program is installed.
//!
//! The native metrics are `native.drafftink_bytes`, `native.mcp_bytes` and `native.server_bytes`: the files that
//! `scripts/package-*` ship, built with the profile `dist` (fat LTO, stripped) and the server with its `sqlite` and
//! `s3` stores. The native run does not belong to `ci`: the build takes about as long as a release.
//!
//! The WebGL2 module for browsers without WebGPU (`dist/pkg-gl`) has its own metrics, `fallback.opt_bytes`,
//! `fallback.gzip_bytes`, `fallback.brotli_bytes` and `fallback.raw_bytes`, with their own limits: it is loaded only by
//! browsers that have no WebGPU, so the normal module's numbers do not include it.
//!
//! The wasm metrics are `wasm.opt_bytes` (the file the server sends), `wasm.gzip_bytes` and `wasm.brotli_bytes` (what goes
//! over the wire; brotli is what browsers ask for), `fonts.brotli_bytes` (the font files, brotli each; the page loads
//! them when it needs them), `schemas.brotli_bytes` (the table of command schemas that the page fetches instead of the
//! module carrying the code that makes them) and, when the build ran, `wasm.raw_bytes` (wasm-bindgen's output before
//! wasm-opt).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::run;
use crate::tools::target_dir;

const BUDGETS: &str = "perf/budgets.json";
const BUILD_SCRIPT: &str = "scripts/build-web.sh";
/// The module wasm-opt wrote, relative to the root.
const OPT_WASM: &str = "dist/pkg/drafftink_web_bg.wasm";
/// The module cargo wrote (it still has the names that `twiggy` needs).
const UNSTRIPPED_WASM: &str = "target/wasm32-unknown-unknown/release-wasm/drafftink_web.wasm";
const FONT_DIR: &str = "dist/fonts";
const SIZES_FILE: &str = "dist/sizes.json";
/// Rows of the `twiggy` listing on a failure.
const TOP_ROWS: &str = "30";

/// What the command line asked for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// Skip the build and measure `dist/`.
    no_build: bool,
    /// A finished module to measure instead of `dist/pkg`.
    wasm: Option<PathBuf>,
    /// Measure the native binaries instead of the web app.
    native: bool,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--no-build" => options.no_build = true,
            "--native" => options.native = true,
            "--wasm" => {
                let path = it.next().ok_or("--wasm needs the path of a .wasm file")?;
                options.wasm = Some(PathBuf::from(path));
            }
            other => return Err(format!("unknown argument `{other}` (see the top of xtask/src/size.rs)")),
        }
    }
    if options.native && options.wasm.is_some() {
        return Err("--native and --wasm measure different things; use one".into());
    }
    Ok(options)
}

/// A limit of the `size` section.
#[derive(Debug, Clone, PartialEq)]
struct Limit {
    max: u64,
    note: String,
}

/// The `size` section of `perf/budgets.json`.
fn parse_budgets(text: &str) -> Result<BTreeMap<String, Limit>, String> {
    let json: Value = serde_json::from_str(text).map_err(|e| format!("{BUDGETS}: {e}"))?;
    let section = json.get("size").and_then(Value::as_object).ok_or(format!("{BUDGETS} has no `size` section"))?;
    section
        .iter()
        .map(|(metric, entry)| {
            let max = entry
                .get("max")
                .and_then(Value::as_u64)
                .ok_or(format!("{BUDGETS}: size.{metric} needs an integer `max`"))?;
            let note = entry.get("note").and_then(Value::as_str).unwrap_or_default().to_owned();
            Ok((metric.clone(), Limit { max, note }))
        })
        .collect()
}

/// Metric name to bytes.
type Metrics = BTreeMap<&'static str, u64>;

/// The limits that `metrics` breaks. A budget on a metric that was not measured is skipped with `skipped` filled in.
fn check(metrics: &Metrics, budgets: &BTreeMap<String, Limit>, skipped: &mut Vec<String>) -> Vec<String> {
    let mut broken = Vec::new();
    for (name, limit) in budgets {
        match metrics.get(name.as_str()) {
            None => skipped.push(name.clone()),
            Some(&value) if value > limit.max => broken.push(format!(
                "{name} is {value} bytes ({:.2} MiB), over the limit of {} ({:.2} MiB). {}",
                mib(value),
                limit.max,
                mib(limit.max),
                limit.note
            )),
            Some(_) => {}
        }
    }
    broken
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn gzip_len(bytes: &[u8]) -> Result<u64, String> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes).map_err(|e| format!("gzip: {e}"))?;
    let out = encoder.finish().map_err(|e| format!("gzip: {e}"))?;
    Ok(out.len() as u64)
}

/// The size of `bytes` at brotli quality 11 and a 4 MiB window, with the Rust encoder. It is about five times slower
/// than the C one (two minutes for the wasm module), so [`brotli_len`] prefers the `brotli` program.
fn brotli_len_in_process(bytes: &[u8]) -> Result<u64, String> {
    let mut out = Vec::new();
    {
        let mut encoder = brotli::CompressorWriter::new(&mut out, 1 << 16, 11, 22);
        encoder.write_all(bytes).map_err(|e| format!("brotli: {e}"))?;
    }
    Ok(out.len() as u64)
}

/// The brotli size of a file: from the `brotli` program when it is installed (what the build script and a
/// production server use, and quick), else from the Rust encoder.
fn brotli_len(path: &Path) -> Result<u64, String> {
    let program = Command::new("brotli").args(["-q", "11", "-c"]).arg(path).stderr(Stdio::null()).output();
    match program {
        Ok(out) if out.status.success() => Ok(out.stdout.len() as u64),
        _ => brotli_len_in_process(&read(path)?),
    }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The font files of `dist/fonts`. Empty when the folder does not exist.
fn font_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = root.join(FONT_DIR);
    let Ok(entries) = fs::read_dir(&dir) else { return Ok(Vec::new()) };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e, "ttf" | "otf")) {
            files.push(path);
        }
    }
    Ok(files)
}

/// Compresses the module and the fonts, one thread each.
fn measure(root: &Path, wasm: &Path) -> Result<Metrics, String> {
    let bytes = read(wasm)?;
    let fonts = font_files(root)?;
    let (gzip, brotli, font_sizes) = std::thread::scope(|scope| {
        let gzip = scope.spawn(|| gzip_len(&bytes));
        let brotli = scope.spawn(|| brotli_len(wasm));
        let font_jobs: Vec<_> = fonts.iter().map(|f| scope.spawn(|| brotli_len(f))).collect();
        let join = |r: std::thread::Result<Result<u64, String>>| {
            r.unwrap_or_else(|_| Err("a compression thread panicked".into()))
        };
        (join(gzip.join()), join(brotli.join()), font_jobs.into_iter().map(|j| join(j.join())).collect::<Vec<_>>())
    });
    let mut metrics = Metrics::new();
    metrics.insert("wasm.opt_bytes", bytes.len() as u64);
    metrics.insert("wasm.gzip_bytes", gzip?);
    metrics.insert("wasm.brotli_bytes", brotli?);
    if !fonts.is_empty() {
        metrics.insert("fonts.brotli_bytes", font_sizes.into_iter().sum::<Result<u64, String>>()?);
    }
    Ok(metrics)
}

/// The shipped binaries and the metric each one is measured as.
const NATIVE_BINARIES: [(&str, &str); 3] = [
    ("drafftink", "native.drafftink_bytes"),
    ("drafftink-mcp", "native.mcp_bytes"),
    ("drafftink-server", "native.server_bytes"),
];

/// The cargo profile of the shipped binaries (`Cargo.toml`, `scripts/package-*`).
const DIST_PROFILE: &str = "dist";

/// Builds the binaries the way `scripts/package-linux.sh` does.
fn build_native(root: &Path) -> Result<(), String> {
    let mut build = crate::cargo(root);
    build.args(["build", "--profile", DIST_PROFILE, "--locked"]);
    for (bin, _) in NATIVE_BINARIES {
        build.args(["-p", bin]);
    }
    // The release builds carry the hand-drawn fonts from the repository (docs/publishing.md).
    build.args([
        "--features",
        "drafftink-server/sqlite,drafftink-server/s3,drafftink/hand-fonts,drafftink-mcp/hand-fonts",
    ]);
    run(&mut build)
}

/// The sizes of the binaries in `dir`.
fn measure_native(dir: &Path) -> Result<Metrics, String> {
    let mut metrics = Metrics::new();
    for (bin, metric) in NATIVE_BINARIES {
        let path = dir.join(format!("{bin}{}", std::env::consts::EXE_SUFFIX));
        let meta = fs::metadata(&path)
            .map_err(|e| format!("{}: {e}. Build it with `cargo xtask size --native`.", path.display()))?;
        metrics.insert(metric, meta.len());
    }
    Ok(metrics)
}

/// The numbers the build script measured with the C compressors (`dist/sizes.json`).
fn from_build_script(root: &Path) -> Result<Metrics, String> {
    let path = root.join(SIZES_FILE);
    let text =
        fs::read_to_string(&path).map_err(|e| format!("{}: {e}. Run scripts/build-web.sh first.", path.display()))?;
    parse_sizes(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_sizes(text: &str) -> Result<Metrics, String> {
    let json: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut metrics = Metrics::new();
    for (key, name) in [
        ("raw", "wasm.raw_bytes"),
        ("opt", "wasm.opt_bytes"),
        ("gzip", "wasm.gzip_bytes"),
        ("brotli", "wasm.brotli_bytes"),
        ("fonts_brotli", "fonts.brotli_bytes"),
        ("schemas_brotli", "schemas.brotli_bytes"),
    ] {
        let value = json.get(key).and_then(Value::as_u64).ok_or(format!("`{key}` is missing or not a whole number"))?;
        metrics.insert(name, value);
    }
    // The WebGL2 module is optional (`build-web.sh --no-fallback` writes zeros): measured only when it was built.
    for (key, name) in [
        ("fallback_raw", "fallback.raw_bytes"),
        ("fallback_opt", "fallback.opt_bytes"),
        ("fallback_gzip", "fallback.gzip_bytes"),
        ("fallback_brotli", "fallback.brotli_bytes"),
    ] {
        if let Some(value) = json.get(key).and_then(Value::as_u64).filter(|v| *v > 0) {
            metrics.insert(name, value);
        }
    }
    Ok(metrics)
}

fn table(metrics: &Metrics, budgets: &BTreeMap<String, Limit>) -> String {
    let mut out = String::from("metric               bytes      MiB     limit (MiB)\n");
    for (name, &value) in metrics {
        let limit = budgets.get(*name).map_or("-".to_owned(), |l| format!("{:.2}", mib(l.max)));
        out.push_str(&format!("{name:<20} {value:>9}  {:>7.2}  {limit:>10}\n", mib(value)));
    }
    out
}

/// The biggest items of the unstripped module, or the reason there is no list.
fn top_list(root: &Path, wasm: &Path) -> String {
    let output = Command::new("twiggy").current_dir(root).args(["top", "-n", TOP_ROWS]).arg(wasm).output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => format!("twiggy failed: {}", String::from_utf8_lossy(&o.stderr)),
        Err(_) => format!(
            "twiggy is not installed (cargo install twiggy --locked); then run: twiggy top -n {TOP_ROWS} {}",
            wasm.display()
        ),
    }
}

pub fn run_gate(root: &Path, args: &[String]) -> Result<(), String> {
    let options = parse_args(args)?;
    let mut budgets =
        parse_budgets(&read(&root.join(BUDGETS)).and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))?)?;
    // The web and the native runs each check their own metrics.
    budgets.retain(|name, _| name.starts_with("native.") == options.native);
    if options.native {
        if !options.no_build {
            build_native(root)?;
        }
        let dir = target_dir(root).join(DIST_PROFILE);
        let metrics = measure_native(&dir)?;
        print!("\n{}", table(&metrics, &budgets));
        let mut skipped = Vec::new();
        let broken = check(&metrics, &budgets, &mut skipped);
        if broken.is_empty() {
            println!("size: the binaries in {} are within the budgets of {BUDGETS}", dir.display());
            return Ok(());
        }
        return Err(format!(
            "{}\nFind what grew with `nm --size-sort -C <binary> | tail -50` on a build without `strip`, or `cargo bloat --profile dist --crates`. Shrink it, or raise the limit in {BUDGETS} with the reason in its `note`.",
            broken.join("\n")
        ));
    }

    let (wasm, unstripped) = match &options.wasm {
        Some(path) => (path.clone(), path.clone()),
        None => {
            if !options.no_build {
                run(Command::new("bash").current_dir(root).arg(BUILD_SCRIPT).arg("--no-precompress"))?;
            }
            (root.join(OPT_WASM), root.join(UNSTRIPPED_WASM))
        }
    };
    let metrics = match options.wasm {
        Some(_) => measure(root, &wasm)?,
        None => from_build_script(root)?,
    };
    print!("\n{}", table(&metrics, &budgets));

    let mut skipped = Vec::new();
    let broken = check(&metrics, &budgets, &mut skipped);
    if !skipped.is_empty() {
        println!(
            "not measured, so not checked: {} (fonts come from {FONT_DIR}/, which scripts/build-web.sh writes)",
            skipped.join(", ")
        );
    }
    if broken.is_empty() {
        println!("size: within the budgets of {BUDGETS}");
        return Ok(());
    }
    println!("\nThe biggest items of {} (before wasm-opt):\n{}", unstripped.display(), top_list(root, &unstripped));
    Err(format!(
        "{}\nFind what grew with the list above (compare with `git stash; cargo xtask size`). Shrink it, or raise the limit in {BUDGETS} with the reason in its `note`.",
        broken.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET_JSON: &str = r#"{"version":1,"scenarios":{},"size":{
        "wasm.brotli_bytes":{"max":3000000,"note":"first load"},
        "fonts.brotli_bytes":{"max":1500000}}}"#;

    #[test]
    fn arguments() {
        assert_eq!(parse_args(&[]).unwrap(), Options::default());
        let o = parse_args(&["--no-build".into(), "--wasm".into(), "a.wasm".into()]).unwrap();
        assert!(o.no_build && o.wasm == Some(PathBuf::from("a.wasm")));
        assert!(parse_args(&["--wasm".into()]).is_err());
        assert!(parse_args(&["--bogus".into()]).unwrap_err().contains("--bogus"));
        assert!(parse_args(&["--native".into()]).unwrap().native);
        assert!(parse_args(&["--native".into(), "--wasm".into(), "a.wasm".into()]).is_err());
    }

    #[test]
    fn the_size_section_is_read() {
        let b = parse_budgets(BUDGET_JSON).unwrap();
        assert_eq!(b["wasm.brotli_bytes"], Limit { max: 3_000_000, note: "first load".into() });
        assert_eq!(b["fonts.brotli_bytes"].note, "");
        assert!(parse_budgets(r#"{"version":1}"#).unwrap_err().contains("no `size` section"));
        assert!(parse_budgets(r#"{"size":{"x":{"note":"no max"}}}"#).unwrap_err().contains("size.x"));
    }

    #[test]
    fn the_numbers_of_the_build_script_are_read() {
        let m = parse_sizes(r#"{"raw": 5, "opt": 4, "gzip": 3, "brotli": 2, "fonts_brotli": 1, "schemas_brotli": 7}"#)
            .unwrap();
        assert_eq!(m["wasm.brotli_bytes"], 2);
        assert_eq!(m["fonts.brotli_bytes"], 1);
        assert_eq!(m["schemas.brotli_bytes"], 7);
        assert_eq!(m.len(), 6, "no fallback numbers, no fallback metrics");
        let both = parse_sizes(
            r#"{"raw": 5, "opt": 4, "gzip": 3, "brotli": 2, "fonts_brotli": 1, "schemas_brotli": 7,
                "fallback_raw": 9, "fallback_opt": 8, "fallback_gzip": 7, "fallback_brotli": 6}"#,
        )
        .unwrap();
        assert_eq!((both["fallback.opt_bytes"], both["fallback.brotli_bytes"], both.len()), (8, 6, 10));
        let skipped = parse_sizes(
            r#"{"raw": 5, "opt": 4, "gzip": 3, "brotli": 2, "fonts_brotli": 1, "schemas_brotli": 7, "fallback_opt": 0, "fallback_brotli": 0}"#,
        )
        .unwrap();
        assert_eq!(skipped.len(), 6, "a zero means the module was not built");
        assert!(parse_sizes(r#"{"raw": 5}"#).unwrap_err().contains("`opt`"));
        assert!(parse_sizes("nonsense").is_err());
    }

    #[test]
    fn the_checked_in_budgets_parse() {
        let text = fs::read_to_string(crate::root().join(BUDGETS)).unwrap();
        let b = parse_budgets(&text).unwrap();
        for name in [
            "wasm.opt_bytes",
            "wasm.gzip_bytes",
            "wasm.brotli_bytes",
            "fonts.brotli_bytes",
            "schemas.brotli_bytes",
            "fallback.opt_bytes",
            "fallback.gzip_bytes",
            "fallback.brotli_bytes",
        ]
        .into_iter()
        .chain(NATIVE_BINARIES.iter().map(|(_, metric)| *metric))
        {
            assert!(b.contains_key(name), "{name}");
        }
    }

    #[test]
    fn the_native_sizes_are_the_sizes_of_the_files() {
        let dir = std::env::temp_dir().join(format!("xtask-size-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert!(measure_native(&dir).unwrap_err().contains("cargo xtask size --native"), "missing binaries are named");
        for (i, (bin, _)) in NATIVE_BINARIES.iter().enumerate() {
            fs::write(dir.join(format!("{bin}{}", std::env::consts::EXE_SUFFIX)), vec![0u8; 100 * (i + 1)]).unwrap();
        }
        let m = measure_native(&dir).unwrap();
        assert_eq!((m["native.drafftink_bytes"], m["native.mcp_bytes"], m["native.server_bytes"]), (100, 200, 300));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_metric_over_its_limit_is_reported_with_the_note() {
        let b = parse_budgets(BUDGET_JSON).unwrap();
        let metrics = Metrics::from([("wasm.brotli_bytes", 3_100_000), ("fonts.brotli_bytes", 1_000_000)]);
        let mut skipped = Vec::new();
        let broken = check(&metrics, &b, &mut skipped);
        assert_eq!(broken.len(), 1);
        assert!(broken[0].contains("wasm.brotli_bytes") && broken[0].contains("first load"), "{broken:?}");
        assert!(skipped.is_empty());
    }

    #[test]
    fn a_limit_is_inclusive_and_an_unmeasured_metric_is_skipped() {
        let b = parse_budgets(BUDGET_JSON).unwrap();
        let metrics = Metrics::from([("wasm.brotli_bytes", 3_000_000)]);
        let mut skipped = Vec::new();
        assert!(check(&metrics, &b, &mut skipped).is_empty());
        assert_eq!(skipped, ["fonts.brotli_bytes"]);
    }

    #[test]
    fn compression_shrinks_repetitive_data_and_the_table_lists_the_limit() {
        let data = vec![7u8; 100_000];
        assert!(gzip_len(&data).unwrap() < 1_000);
        assert!(brotli_len_in_process(&data).unwrap() < 200);
        let b = parse_budgets(BUDGET_JSON).unwrap();
        let t = table(&Metrics::from([("wasm.brotli_bytes", 2_097_152)]), &b);
        assert!(t.contains("wasm.brotli_bytes") && t.contains("2.00") && t.contains("2.86"), "{t}");
    }
}
