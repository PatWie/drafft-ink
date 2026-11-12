//! The subcommands of `drafftink`: `mcp`, and the names that moved to `drafftink-headless`.
//!
//! `drafftink` is the program with a screen, so it keeps only what talks to the window: `drafftink mcp` is the bridge
//! between an AI agent and the running app (or a browser tab with `--relay`). The tools that need no screen are in
//! the `drafftink-headless` package, which links no GPU or window code. A user who types one of them here gets one
//! line that says where it went, and a non-zero exit code, not a window and not a usage error.

use std::ffi::OsString;
use std::process::ExitCode;

use clap::Subcommand;
use drafftink_mcp::cli::BridgeArgs;

/// The name that errors start with.
const PROGRAM: &str = "drafftink";

/// The program that has the tools without a screen.
const HEADLESS: &str = "drafftink-headless";

/// The exit code of a command that is not part of this program: the same as a mistake in the arguments.
const MOVED: u8 = 2;

/// A subcommand of `drafftink`. The first word decides: a file named like one is given as `./run` or after `--`.
#[derive(Debug, Subcommand)]
pub enum Tool {
    /// Serve MCP on standard input and output for an AI agent, driving the app that is open.
    ///
    /// Attaches to the running desktop app by default, so the user watches the agent draw (start the app with
    /// `--bridge`, or switch it on in the preferences). `--relay` drives a browser tab instead. For an agent that
    /// needs no app, see `drafftink-headless mcp`.
    Mcp(McpCommand),
    /// Moved to `drafftink-headless`.
    #[command(hide = true, disable_help_flag = true)]
    Convert(Moved),
    /// Moved to `drafftink-headless`.
    #[command(hide = true, disable_help_flag = true)]
    Run(Moved),
    /// Moved to `drafftink-headless`.
    #[command(hide = true, disable_help_flag = true)]
    Outline(Moved),
    /// Moved to `drafftink-headless`.
    #[command(hide = true, disable_help_flag = true)]
    Inspect(Moved),
    /// Moved to `drafftink-headless`.
    #[command(hide = true, disable_help_flag = true)]
    Commands(Moved),
    /// Moved to `drafftink-headless`.
    #[command(hide = true, disable_help_flag = true)]
    Schemas(Moved),
}

/// The arguments of `drafftink mcp`: the bridge modes, and the options of the headless mode, which only lead to the
/// message that it moved.
#[derive(Debug, clap::Args)]
pub struct McpCommand {
    /// `--attach` (the default), `--relay` and their options.
    #[command(flatten)]
    bridge: BridgeArgs,
    #[arg(long = "allow-dir", value_name = "DIR", hide = true)]
    allow_dir: Vec<OsString>,
    #[arg(long, hide = true)]
    allow_any_path: bool,
}

impl McpCommand {
    /// Whether the options ask for the headless mode, which is not part of this program.
    fn asks_for_headless(&self) -> bool {
        !self.allow_dir.is_empty() || self.allow_any_path
    }
}

/// Everything after the name of a tool that moved. It is read and ignored, so that any arguments reach the message
/// instead of a usage error.
#[derive(Debug, clap::Args)]
pub struct Moved {
    #[arg(num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    _rest: Vec<OsString>,
}

impl Tool {
    /// Runs the tool to its end and gives the exit code of the process.
    pub fn run(&self) -> ExitCode {
        let moved = match self {
            Self::Mcp(command) if command.asks_for_headless() => "mcp",
            Self::Mcp(command) => return drafftink_mcp::cli::run_bridge(&command.bridge, PROGRAM),
            Self::Convert(_) => "convert",
            Self::Run(_) => "run",
            Self::Outline(_) => "outline",
            Self::Inspect(_) => "inspect",
            Self::Commands(_) => "commands",
            Self::Schemas(_) => "schemas",
        };
        eprintln!("{}", moved_message(moved));
        ExitCode::from(MOVED)
    }
}

/// The one line for a tool that is in `drafftink-headless`.
fn moved_message(subcommand: &str) -> String {
    format!("That is part of {HEADLESS}: `cargo install {HEADLESS}`, then `{HEADLESS} {subcommand} …`")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_names_the_program_the_install_command_and_the_subcommand() {
        assert_eq!(
            moved_message("convert"),
            "That is part of drafftink-headless: `cargo install drafftink-headless`, then `drafftink-headless convert …`"
        );
    }
}
