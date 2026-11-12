//! `drafftink-headless`: Drafft.ink without a screen.
//!
//! ```text
//! drafftink-headless serve [--bind ADDR] [--web-dir DIR] [--storage KIND] ...   the collaboration server
//! drafftink-headless mcp [--attach | --relay | --allow-dir DIR ...]             MCP on stdio
//! drafftink-headless convert|run|outline|inspect|commands|schemas ...           documents, no window
//! ```
//!
//! Every subcommand is the code of a library crate that this program only puts under one name: `drafftink-server`
//! (`serve`), `drafftink-mcp` (`mcp`) and `drafftink-cli` (the rest). The program links no GPU, window, egui or
//! file-dialog code, so it runs over SSH, in CI and in a container (`cargo xtask layers` checks the dependencies).
//! The desktop app is `drafftink`; its `drafftink mcp` is the bridge to a running window.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use drafftink_cli::cli::Command as DocumentCommand;
use drafftink_mcp::cli::Args as McpArgs;
use drafftink_server::Args as ServeArgs;

/// The name that errors start with.
const PROGRAM: &str = "drafftink-headless";

/// Drafft.ink without a screen: the collaboration server, an MCP server and tools for documents.
///
/// Run with a subcommand; without one this help is printed and nothing starts. Documents are drafft.ink,
/// Excalidraw, Mermaid and SVG files; every operation is a command of the registry, the same one the app runs.
#[derive(Debug, Parser)]
#[command(name = "drafftink-headless", version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the collaboration server, which also serves the web app.
    ///
    /// Listens on 127.0.0.1:3030 unless `--bind` says otherwise. Every flag has an environment variable of the same
    /// name with the `DRAFFTINK_` prefix. Stops on Ctrl-C or SIGTERM after it has saved every room.
    Serve(Box<ServeArgs>),
    /// Serve MCP on standard input and output for an AI agent.
    ///
    /// With no mode flag the agent works on a document of its own. `--attach` drives the running desktop app and
    /// `--relay` a browser tab, as `drafftink mcp` does.
    Mcp(McpArgs),
    /// The tools for documents: run a script, convert, outline, inspect, list the commands.
    #[command(flatten)]
    Document(DocumentCommand),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Serve(args) => drafftink_server::run(*args),
        Command::Mcp(args) => drafftink_mcp::cli::run_server(&args, PROGRAM),
        Command::Document(command) => drafftink_cli::cli::run_command(&command, PROGRAM),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once(PROGRAM).chain(args.iter().copied()))
    }

    #[test]
    fn the_command_line_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_subcommand_is_reached() {
        for args in [
            &["serve"][..],
            &["serve", "--bind", "127.0.0.1:0", "--storage", "memory"],
            &["mcp"],
            &["mcp", "--attach", "--port", "5"],
            &["mcp", "--relay"],
            &["mcp", "--allow-dir", "."],
            &["convert", "a.drafftink", "b.excalidraw"],
            &["run", "script.jsonl"],
            &["outline", "a.drafftink"],
            &["inspect", "a.drafftink"],
            &["commands"],
            &["schemas"],
        ] {
            assert!(parse(args).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn no_subcommand_prints_help_and_is_a_usage_error() {
        let err = parse(&[]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand);
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn help_lists_every_subcommand_on_one_line() {
        let help = Cli::command().render_help().to_string();
        for name in ["serve", "mcp", "run", "convert", "outline", "inspect", "commands", "schemas"] {
            let line = help.lines().find(|l| l.trim_start().starts_with(&format!("{name} "))).unwrap_or("");
            assert!(!line.is_empty() && line.len() < 120, "{name}: {line:?}");
        }
    }

    #[test]
    fn a_mode_option_without_its_mode_is_refused_as_it_was_in_drafftink_mcp() {
        for args in [&["mcp", "--attach", "--relay"][..], &["mcp", "--port", "5"], &["mcp", "--relay-port", "5"]] {
            assert_eq!(parse(args).unwrap_err().exit_code(), 2, "{args:?}");
        }
    }
}
