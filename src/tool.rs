//! The headless subcommands: `drafftink convert|run|outline|inspect|commands|schemas` and `drafftink mcp`.
//!
//! They are the code of the standalone `drafftink-cli` and `drafftink-mcp` binaries, not a copy: the argument
//! definitions and the work live in those two crates and this module only puts them under one name. Nothing here
//! touches the window, the GPU or the display, so the tools run over SSH, in CI and in a container.

use std::process::ExitCode;

use clap::Subcommand;
use drafftink_cli::cli::Command as CliCommand;
use drafftink_mcp::cli::Args as McpArgs;

/// The name that errors start with.
const PROGRAM: &str = "drafftink";

/// A tool that runs without a window.
#[derive(Debug, Subcommand)]
pub enum Tool {
    /// The commands of `drafftink-cli`, with the same arguments, output and exit codes.
    #[command(flatten)]
    Cli(CliCommand),
    /// Serve MCP on standard input and output for an AI agent.
    ///
    /// Headless by default, `--attach` to the running app, or `--relay` to a browser tab. Same as `drafftink-mcp`.
    Mcp(McpArgs),
}

impl Tool {
    /// Runs the tool to its end and gives the exit code of the process.
    pub fn run(&self) -> ExitCode {
        match self {
            Self::Cli(command) => drafftink_cli::cli::run_command(command, PROGRAM),
            Self::Mcp(args) => drafftink_mcp::cli::run_server(args, PROGRAM),
        }
    }
}
