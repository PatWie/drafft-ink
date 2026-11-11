//! The desktop scenario runner (`--perf-scenario`, `docs/performance.md` section 5.3).
//!
//! A thread next to the event loop puts the generated scene on screen, waits until the app has drawn it and gone
//! quiet, resets the profiler and prints `PERF-READY` on stdout. The window that follows is the measurement.
//! The input comes from outside as real operating system events (`scripts/perf_scenarios.py` uses X11 XTEST).
//! The window ends when the runner reads a line from stdin, or after `secs` when the launcher gave a length.
//! Then the runner writes the [`ScenarioReport`], including the CPU time the process used in the window, and
//! closes the app.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use drafftink_profile::{ScenarioKey, ScenarioReport};
use thiserror::Error;

use crate::platform::WakeCell;
use crate::{SceneLoader, scene_for};

/// How long the app must draw nothing before the window opens.
const QUIET: Duration = Duration::from_millis(1_000);
/// Longest wait for the scene to load and the app to go quiet. A 100,000-element scene takes several seconds.
const SETTLE_LIMIT: Duration = Duration::from_secs(180);
/// The kernel's `USER_HZ`: ticks per second in `/proc/<pid>/stat`.
const TICKS_PER_SECOND: f64 = 100.0;

/// What to run and where the report goes.
#[derive(Debug, Clone)]
pub struct ScenarioRun {
    /// The scenario, such as `pan@10k`.
    pub key: ScenarioKey,
    /// Where to write the [`ScenarioReport`] JSON.
    pub out: PathBuf,
    /// Length of the window. `None`: until a line arrives on stdin.
    pub secs: Option<f64>,
}

/// Why a run could not produce a report.
#[derive(Debug, Error)]
enum RunError {
    #[error("the scene could not be generated: {0}")]
    Scene(#[from] drafftink_scene::scenegen::GenError),
    #[error("the app did not install the scene within {} s", SETTLE_LIMIT.as_secs())]
    NotInstalled,
    #[error("could not write the report to {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
}

/// Starts the runner thread. The returned handle can be dropped; the thread ends the app when it is done.
pub fn spawn(run: ScenarioRun, loader: SceneLoader, wake: WakeCell) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(e) = drive(&run, &loader) {
            eprintln!("drafftink: scenario {}: {e}", run.key);
        }
        wake.exit();
    })
}

/// Process CPU time (user plus system, all threads) in seconds, from `/proc/self/stat`.
fn cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The command name may hold spaces and parentheses; the numbers follow the last `)`.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / TICKS_PER_SECOND)
}

/// Seconds from the start of this process to now, from the start time in `/proc/self/stat` (field 22, in ticks since
/// boot) and `/proc/uptime`. The resolution is one tick, 10 ms.
fn process_age_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let started: f64 = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()?;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    Some(uptime - started / TICKS_PER_SECOND)
}

/// Waits for the first frame and returns the age of the process at that moment.
fn wait_first_frame() -> Option<f64> {
    let start = Instant::now();
    while drafftink_gpu::frames_drawn() == 0 {
        if start.elapsed() > SETTLE_LIMIT {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    process_age_seconds()
}

fn wait_quiet() {
    let start = Instant::now();
    let mut last = drafftink_gpu::frames_drawn();
    let mut since = Instant::now();
    while start.elapsed() < SETTLE_LIMIT {
        std::thread::sleep(Duration::from_millis(50));
        let now = drafftink_gpu::frames_drawn();
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= QUIET {
            return;
        }
    }
}

fn drive(run: &ScenarioRun, loader: &SceneLoader) -> Result<(), RunError> {
    let first_frame = wait_first_frame();
    let mut scene_ms = None;
    if let Some(install) = scene_for(&run.key)? {
        let before = loader.applied();
        let frames = drafftink_gpu::frames_drawn();
        let start = Instant::now();
        loader.load(install);
        while loader.applied() == before {
            if start.elapsed() > SETTLE_LIMIT {
                return Err(RunError::NotInstalled);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // Installed. The scene is on screen with the next frame that follows.
        while drafftink_gpu::frames_drawn() == frames && start.elapsed() < SETTLE_LIMIT {
            std::thread::sleep(Duration::from_millis(2));
        }
        scene_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
    }
    wait_quiet();

    // The start-up spans (`startup_*`) are in the report as external numbers; the reset drops them from the stats.
    let startup: Vec<(String, f64)> = drafftink_profile::snapshot()
        .spans
        .into_iter()
        .filter(|s| s.name.starts_with("startup_"))
        .map(|s| (format!("{}_ms", s.name), s.total_ms))
        .collect();
    drafftink_profile::reset();
    let (cpu0, t0) = (cpu_seconds(), Instant::now());
    println!("PERF-READY");
    // A closed stdout means nobody listens; the run is then self-timed or interactive.
    let _ = std::io::stdout().flush();
    match run.secs {
        Some(secs) => std::thread::sleep(Duration::from_secs_f64(secs.max(0.0))),
        None => {
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);
        }
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let cpu = cpu_seconds().zip(cpu0).map(|(end, start)| end - start);

    let mut report = run.key.report("native", drafftink_profile::snapshot());
    report.meta.insert("build".into(), if cfg!(debug_assertions) { "debug" } else { "release" }.into());
    report.external.extend(startup);
    report.external.insert("window_s".into(), elapsed);
    if let Some(age) = first_frame {
        report.external.insert("first_frame_ms".into(), age * 1000.0);
    }
    if let Some(ms) = scene_ms {
        report.external.insert("scene_to_frame_ms".into(), ms);
    }
    if let Some(cpu) = cpu.filter(|_| elapsed > 0.0) {
        report.external.insert("cpu_pct".into(), cpu / elapsed * 100.0);
    }
    write_report(&run.out, &report)
}

fn write_report(path: &PathBuf, report: &ScenarioReport) -> Result<(), RunError> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, report.to_json_pretty()).map_err(|source| RunError::Write { path: path.clone(), source })
}
