//! `drafftink-cli`: a headless command line over the editor engine. See `docs/cli.md`.

use std::process::ExitCode;

fn main() -> ExitCode {
    drafftink_cli::cli::main()
}
