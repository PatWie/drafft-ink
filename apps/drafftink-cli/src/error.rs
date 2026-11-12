//! What can go wrong, as one type with plain-words messages.

use std::path::PathBuf;

use drafftink_engine::Refusal;

use crate::script::ScriptError;

/// Why a command line run failed. The process exits with 1 for all of these.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// A file could not be read.
    #[error("could not read `{}`: {source}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// A file could not be written.
    #[error("could not write `{}`: {source}", path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// Standard input could not be read.
    #[error("could not read standard input: {0}")]
    Stdin(std::io::Error),
    /// Output could not be written.
    #[error("could not write the output: {0}")]
    Output(#[from] std::io::Error),
    /// The script is not a script.
    #[error(transparent)]
    Script(#[from] ScriptError),
    /// A command refused. The text says how to go on.
    #[error("{}", .0.message)]
    Refused(Box<Refusal>),
    /// A step refused, and the run stopped there.
    #[error("step {step} refused: {message}")]
    StepRefused {
        /// The step, counted from 1.
        step: usize,
        /// The words of the refusal.
        message: String,
    },
    /// Some steps of a script that ran with `--keep-going` refused.
    #[error("{0} step(s) refused")]
    StepsRefused(usize),
    /// The file extension or `--from` / `--to` does not name a format.
    #[error("{0}")]
    Format(String),
    /// PNG was asked for, and this build has no renderer.
    #[error(
        "this build of the command line cannot write PNG. Export SVG instead (`--to svg`), or build it with the `png` feature (`cargo install drafftink-cli --features png`), or export from the desktop app."
    )]
    NoPng,
    /// A result could not be turned into JSON.
    #[error("could not format a result: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<Refusal> for CliError {
    fn from(r: Refusal) -> Self {
        Self::Refused(Box::new(r))
    }
}
