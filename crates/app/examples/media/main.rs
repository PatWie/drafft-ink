//! Renders the README pictures from the real app, offscreen: `cargo xtask media`.
//!
//! The list of pictures is `docs/media/shots.json`. A *window* shot is the whole app (the UI over the canvas, the
//! intro document, a selection chosen by commands) drawn by `drafftink_gpu::headless`. An *export* shot is the
//! document alone through the PNG export. See `docs/media.md`.
// A native tool: the wall clock is allowed (the rule against `Instant::now` is for code that also runs on the web).
#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr)]

mod png_out;
mod render;
mod spec;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;

/// Renders the pictures of the shot list into a folder.
#[derive(Debug, Parser)]
struct Cli {
    /// The shot list.
    #[arg(long, default_value = "docs/media/shots.json")]
    shots: PathBuf,
    /// Where the PNG files go.
    #[arg(long, default_value = "docs/img")]
    out: PathBuf,
    /// Render only the shots with these names.
    #[arg(long = "only", value_name = "NAME")]
    only: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Spec(#[from] spec::SpecError),
    #[error(transparent)]
    Render(#[from] render::RenderError),
    #[error("cannot write {path}: {source}")]
    Write { path: String, source: std::io::Error },
    #[error("there is no shot named {0}. The list has: {1}")]
    UnknownShot(String, String),
}

fn run(cli: &Cli) -> Result<(), Error> {
    let shots = spec::load(&cli.shots)?;
    if let Some(unknown) = cli.only.iter().find(|n| shots.iter().all(|s| &s.name != *n)) {
        let known: Vec<&str> = shots.iter().map(|s| s.name.as_str()).collect();
        return Err(Error::UnknownShot(unknown.clone(), known.join(", ")));
    }
    std::fs::create_dir_all(&cli.out).map_err(|source| Error::Write { path: cli.out.display().to_string(), source })?;
    for shot in shots.iter().filter(|s| cli.only.is_empty() || cli.only.contains(&s.name)) {
        let started = Instant::now();
        let png = render::render(shot)?;
        let path = cli.out.join(format!("{}.png", shot.name));
        std::fs::write(&path, &png).map_err(|source| Error::Write { path: path.display().to_string(), source })?;
        println!("{}: {} KB in {:.1} s", path.display(), png.len().div_ceil(1024), started.elapsed().as_secs_f64());
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(&Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("media: {e}");
            ExitCode::FAILURE
        }
    }
}
